pub mod methods;

use tokio::net;

use std::collections::HashMap;
use std::sync::Arc;

use crate::config;
use crate::files::Files;
use crate::proto::{self, Method, RawMethod};

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
}

pub struct Context {
    pub num: u64,
    pub methods: HashMap<&'static str, &'static dyn RawMethod>
}

impl Context {
    pub fn init() -> Context {
        Context {
            num: 0,
            methods: HashMap::new()
        }
    }

    pub fn register_method<M: Method>(&mut self, method: &'static M) {
        self.methods.insert(M::NAME, method);
    }
}

impl Server {
    // initialise the server with a given configuration
    pub async fn init(config: config::Server) -> anyhow::Result<Server> {
        log::info!("initialising server with config: {config:#?}");
        let listener = net::TcpListener::bind(config.addr).await?;
        log::info!("listener bound to {}", config.addr);
        let files = Files::open(config.db)?;

        Ok(Server { listener, files: Arc::new(files) })
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            // accept connection from tcp listener
            let (mut stream, addr) = self.listener.accept().await?;
            log::info!("handling connection from {addr}");

            // clone resources for connection handler
            let files = self.files.clone();

            tokio::spawn(async move {
                let mut ctx = Context::init();

                ctx.register_method(&methods::Get);
                ctx.register_method(&methods::Insert);
                ctx.register_method(&methods::GetListing);
                ctx.register_method(&methods::Clear);
                ctx.register_method(&methods::Delete);
                ctx.register_method(&methods::Rollback);
                ctx.register_method(&methods::GetNode);
                ctx.register_method(&methods::GetHistory);
                ctx.register_method(&methods::GetCtx);
                ctx.register_method(&methods::Increment);

                // wrap operation in async block; lets us catch errors
                let fut = async move {
                    loop { 
                        // read next packet from client
                        if let Some(request) = proto::read_packet(&mut stream).await? {
                            // dispatch request to respective method handler
                            let handler = ctx.methods.get(&request.method[..])
                                .ok_or(Error::InvalidMethod(request.method))?;

                            let response = handler.call_bytes(&files, &mut ctx, request.data);

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

