pub mod methods;

use tokio::net;

use std::collections::HashMap;
use std::sync::Arc;

use crate::config;
use crate::files::Files;
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
    methods: Arc<HashMap<&'static str, MethodFn>>,
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
    pub async fn build(self, config: config::Server) -> anyhow::Result<Server> {
        log::info!("initialising server with config: {config:#?}");
        let listener = net::TcpListener::bind(config.addr).await?;
        log::info!("listener bound to {}", config.addr);
        let files = Files::open(config.db)?;

        Ok(Server { listener, files: Arc::new(files), methods: Arc::new(self.methods) })
    }
}

pub struct Context(u64);

impl Server {
    // initialise the server with a given configuration
    pub async fn init(config: config::Server) -> anyhow::Result<Server> {
        ServerBuilder::new()
            // install method handlers we need
            .add(methods::Get)
            .add(methods::Insert)
            .add(methods::GetListing)
            .add(methods::Clear)
            .add(methods::Delete)
            .add(methods::Rollback)
            .add(methods::GetNode)
            .add(methods::GetHistory)
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
                let mut ctx = Context(0);

                // wrap operation in async block; lets us catch errors
                let fut = async move {
                    loop { 
                        // read next packet from client
                        if let Some(request) = proto::read_packet(&mut stream).await? {
                            // dispatch request to respective method handler
                            let handler = methods.get(&request.method[..])
                                .ok_or(Error::InvalidMethod(request.method))?;

                            let response = handler(&files, &mut ctx, request.data);

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

