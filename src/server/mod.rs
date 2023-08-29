pub mod methods;

use tokio::net::{self, TcpStream};

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::config;
use crate::files::Files;
use crate::proto::{self, Method, Packet, RawMethod};

/// Represents the different protocol-specific errors that can be encountered while the server is running
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // #[error("invalid bincode {0:?}")]
    // InvalidBincode(Vec<u8>),
    #[error("invalid method {0:?}")]
    InvalidMethod(String),
}

/// The [Server] struct holds all the state required to serve requests for files
pub struct Server {
    listener: net::TcpListener,
    files: Arc<Files>,
}

pub struct Context {
    addr: SocketAddr,
    methods: HashMap<&'static str, &'static dyn RawMethod>,
    stream: TcpStream,
}

impl Context {
    fn init(addr: SocketAddr, stream: TcpStream) -> Context {
        Context {
            addr,
            stream,
            methods: HashMap::new(),
        }
    }

    /// Returns the socket address a [Context] concerns
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Register a given [Method] with the [Context]
    pub fn register<M: Method>(&mut self, method: &'static M) {
        self.methods.insert(M::NAME, method);
    }

    /// De-registers a given [Method] from the [Context]
    pub fn deregister<M: Method>(&mut self, _method: &'static M) {
        self.methods.remove(M::NAME);
    }

    /// Calls the respective method for a context based on a request [Packet]
    pub fn dispatch(&mut self, files: &Files, packet: Packet) -> anyhow::Result<Vec<u8>> {
        // dispatch request to respective method handler
        let handler = self
            .methods
            .get(&packet.method[..])
            .ok_or(Error::InvalidMethod(packet.method))?;

        handler.call_bytes(files, self, packet.data)
    }
}

impl Server {
    // initialise the server with a given configuration
    pub async fn init(config: config::Server) -> anyhow::Result<Server> {
        log::info!("initialising server with config: {config:#?}");
        let listener = net::TcpListener::bind(config.addr).await?;
        log::info!("listener bound to {}", config.addr);
        let files = Files::open(config.db)?;

        Ok(Server {
            listener,
            files: Arc::new(files),
        })
    }

    async fn accept(&self) -> io::Result<Context> {
        let (stream, addr) = self.listener.accept().await?;

        let mut context = Context::init(addr, stream);

        if self.files.is_configured() {
            context.register(&methods::auth::Identify);
        } else {
            log::warn!("server has not been configured");
            context.register(&methods::admin::Configure);
        }

        Ok(context)
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            // accept connection from tcp listener
            let mut ctx = self.accept().await?;
            let addr = ctx.addr();

            let files = self.files.clone();

            tokio::spawn(async move {
                // wrap operation in async block; lets us catch errors
                let fut = async move {
                    loop {
                        // read next packet from client
                        if let Some(request) = proto::read_packet(&mut ctx.stream).await? {
                            // dispatch request to respective method handler
                            let response = ctx.dispatch(&files, request);

                            // send response to client
                            match response {
                                Ok(data) => {
                                    // proto::write_packet(&mut stream, "OK", data).await?;
                                    proto::Packet {
                                        method: "OK".to_string(),
                                        data,
                                    }
                                    .write(&mut ctx.stream)
                                    .await?;
                                }

                                Err(e) => {
                                    proto::write_packet(&mut ctx.stream, "ERR", e.to_string())
                                        .await?;
                                    return Err(e);
                                }
                            }
                        } else {
                            break;
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
