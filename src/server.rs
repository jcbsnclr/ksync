use tokio::net;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use crate::files::Files;
use crate::config::Config;
use crate::files::Path;
use crate::proto::{self, Method, MethodFn};

/// Represents the different protocol-specific errors that can be encountered while the server is running
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // #[error("invalid bincode {0:?}")]
    // InvalidBincode(Vec<u8>),
    #[error("invalid method {0:?}")]
    InvalidMethod(String)
}

/// The [Server] struct holds all the state required to serve requests for files
pub struct Server {
    listener: net::TcpListener,
    files: Arc<Files>,
    /// A map of a [Method]'s [Method::NAME] to it's [Method::call_bytes] implementation. Used to service requests
    methods: Arc<HashMap<&'static str, MethodFn>>
}

/// Used to construct a [Server].
pub struct ServerBuilder {
    methods: HashMap<&'static str, MethodFn>
}

impl ServerBuilder {
    pub fn new() -> ServerBuilder {
        ServerBuilder { methods: HashMap::new() }
    }

    /// Register a [Method] with the server
    pub fn add<M: Method>(mut self, _: M) -> ServerBuilder {
        self.methods.insert(M::NAME, M::call_bytes);
        self
    }

    /// Construct the [Server] object
    pub async fn build(self, config: Config) -> anyhow::Result<Server> {
        log::info!("initialising server with config: {config:#?}");
        let listener = net::TcpListener::bind(config.server.addr).await?;
        log::info!("listener bound to {}", config.server.addr);
        let files = Files::open(config.server.db)?;

        Ok(Server { listener, files: Arc::new(files), methods: Arc::new(self.methods) })
    }
}

impl Server {
    // initialise the server with a given configuration
    pub async fn init(config: Config) -> anyhow::Result<Server> {
        ServerBuilder::new()
            // install method handlers we need
            .add(Get)
            .add(Insert)
            .build(config).await
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            // accept connection from tcp listener
            let (mut stream, addr) = self.listener.accept().await?;
            log::info!("handling connection from {addr}");

            // clone resources for connection handler
            let files = self.files.clone();
            let methods = self.methods.clone();

            tokio::spawn(async move {
                // wrap operation in async block; lets us catch errors
                let fut = async move {
                    loop { 
                        // read next packet from client
                        if let Some(request) = proto::read_packet(&mut stream).await? {
                            // dispatch request to respective method handler
                            let handler = methods.get(&request.method[..])
                                .ok_or(Error::InvalidMethod(request.method))?;

                            let response = handler(&files, request.data);

                            // send response to client
                            match response {
                                Ok(data) => {
                                    // proto::write_packet(&mut stream, "OK", data).await?;
                                    proto::Packet {
                                        method: "OK".to_string(),
                                        data
                                    }.write(&mut stream).await?;
                                },

                                Err(e) => {
                                    proto::write_packet(&mut stream, "ERR", e.to_string()).await?;
                                    return Err(e)
                                }
                            }
                        } else {
                            break
                        }   
                    }

                    log::info!("connection with {addr} closed");
                    Ok(())
                };

                // log errors
                match fut.await {
                    Ok(()) => (),
                    Err(e) => {
                        log::error!("error processing stream {addr}: {e}")
                    }
                }
            });
        }
    }
}

/// The [Get] method resolves a virtual filesystem [Path] to it's respective object, loads it, and sends it back to the client
pub struct Get;

impl Method for Get {
    type Input<'a> = Path<'a>;
    type Output = Vec<u8>;

    const NAME: &'static str = "GET";

    fn call<'a>(files: &Files, path: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("retrieving file {path}");

        let object = files.with_root("root", |node| {
            if let Some(&mut object) = node.traverse(path)?.file() {
                Ok(object)
            } else {
                let err: io::Error = io::ErrorKind::InvalidInput.into();
                Err(err.into())
            }
        })?;

        log::info!("got object {}; returning", object.hex());

        let data = files.get(&object)?;

        Ok((&data[..]).to_owned())
    }
}

pub struct Insert;

impl Method for Insert {
    type Input<'a> = (Path<'a>, Vec<u8>);
    type Output = ();

    const NAME: &'static str = "INSERT";

    fn call<'a>(files: &Files, (path, data): Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("storing file {path}");
        log::info!("file contents: {:?}", String::from_utf8_lossy(&data));
        
        let (parent, _) = path.parent_child();

        let object = files.create_object(&data)?;

        files.with_root("root", |node| {
            node.make_dir_recursive(parent)?;
            node.insert(path, object)?;
            Ok(())
        })?;
        log::info!("stored {path} (object {})", object.hex());

        Ok(())
    }
}