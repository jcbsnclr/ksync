use tokio::net;

use serde::{Serialize, Deserialize};

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use crate::files::Files;
use crate::config::Config;

use common::{proto, Path};

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

/// A type signature representing an implementation of [Method::call_bytes]
type MethodFn = fn(&Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>>;

/// The [Method] trait is used to implement different methods of the protocol (e.g. `GET`, `INSERT`, etc.)
pub trait Method {
    type Input<'a>: Deserialize<'a>;
    type Output: Serialize;

    /// The 8-byte identifier of the method. Used to dynamically dispatch a request to it's responder
    const NAME: &'static str;

    /// A wrapper over [Method::call] that deserialises input, and serialises output, automatically
    fn call_bytes(files: &Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let input = bincode::deserialize(&bytes)?;
        let output = Self::call(files, input)?;
        let output_bytes = bincode::serialize(&output)?;

        Ok(output_bytes)
    }

    /// The functionality to be invoked when a method is called
    fn call<'a>(files: &Files, input: Self::Input<'a>) -> anyhow::Result<Self::Output>;
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
    pub async fn init(config: Config) -> anyhow::Result<Server> {
        ServerBuilder::new()
            .add(Get)
            .add(Insert)
            .build(config).await
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            let (mut stream, addr) = self.listener.accept().await?;
            let (files, methods) = (self.files.clone(), self.methods.clone());
            log::info!("handling connection from {addr}");

            tokio::spawn(async move {
                let fut = async move {
                    loop {
                        if let Some(request) = proto::read_packet(&mut stream).await? {
                            let handler = methods.get(&request.method[..])
                                .ok_or(Error::InvalidMethod(request.method))?;

                            let response = handler(&files, request.data);

                            match response {
                                Ok(data) => {
                                    proto::write_packet(&mut stream, "OK", data).await?;
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

struct Get;

impl Method for Get {
    type Input<'a> = Path<'a>;
    type Output = Vec<u8>;

    const NAME: &'static str = "GET";

    fn call<'a>(files: &Files, path: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("retrieving file {path}");
        let object = files.lookup(path)?
            .ok_or::<io::Error>(io::ErrorKind::NotFound.into())?;
        log::info!("got object {}; returning", object.hex());

        let data = files.get(&object)?;

        Ok((&data[..]).to_owned())
    }
}

struct Insert;

impl Method for Insert {
    type Input<'a> = (Path<'a>, Vec<u8>);
    type Output = ();

    const NAME: &'static str = "INSERT";

    fn call<'a>(files: &Files, (path, data): Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("storing file {path}");
        log::info!("file contents: {:?}", String::from_utf8_lossy(&data));
        
        let (parent, _) = path.parent_child();

        files.make_dir_recursive(parent)?;
        let object = files.insert(path, &data)?;
        log::info!("stored {path} (object {})", object.hex());

        Ok(())
    }
}