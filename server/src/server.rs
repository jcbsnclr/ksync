use tokio::net;

use serde::{Serialize, Deserialize};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path;
use std::io;
use std::sync::Arc;

use crate::files::Files;
use common::proto;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    // #[error("invalid bincode {0:?}")]
    // InvalidBincode(Vec<u8>),
    #[error("invalid method {0:?}")]
    InvalidMethod([u8; 8])
}

pub struct Server {
    listener: net::TcpListener,
    files: Arc<Files>,
    methods: Arc<HashMap<[u8; 8], MethodFn>>
}

#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    pub addr: SocketAddr,
    pub db: path::PathBuf
}

type MethodFn = fn(&Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>>;

pub trait Method {
    type Input<'a>: Deserialize<'a>;
    type Output: Serialize;

    const NAME: [u8; 8];

    fn call_bytes(files: &Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let input = bincode::deserialize(&bytes)?;
        let output = Self::call(files, input)?;
        let output_bytes = bincode::serialize(&output)?;

        Ok(output_bytes)
    }

    fn call<'a>(files: &Files, input: Self::Input<'a>) -> anyhow::Result<Self::Output>;
}

pub struct ServerBuilder {
    methods: HashMap<[u8; 8], MethodFn>
}

impl ServerBuilder {
    pub fn new() -> ServerBuilder {
        ServerBuilder { methods: HashMap::new() }
    }

    pub fn add<M: Method>(mut self, _: M) -> ServerBuilder {
        self.methods.insert(M::NAME, M::call_bytes);
        self
    }

    pub async fn build(self, config: ServerConfig) -> anyhow::Result<Server> {
        log::info!("initialising server with config: {config:#?}");
        let listener = net::TcpListener::bind(config.addr).await?;
        log::info!("listener bound to {}", config.addr);
        let files = Files::open(config.db)?;

        files.clear()?;

        Ok(Server { listener, files: Arc::new(files), methods: Arc::new(self.methods) })
    }
}

impl Server {
    pub async fn init(config: ServerConfig) -> anyhow::Result<Server> {
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
                    let request = proto::read_packet(&mut stream).await?;
                    // dbg!(&request);

                    let handler = methods.get(&request.method)
                        .ok_or(Error::InvalidMethod(request.method))?;

                    let response = handler(&files, request.data);

                    match response {
                        Ok(data) => {
                            let response = proto::Packet {
                                method: b"OK\0\0\0\0\0\0".to_owned(),
                                data
                            };

                            response.write(&mut stream).await?;

                            Ok::<_, anyhow::Error>(())
                        },

                        Err(e) => {
                            let response = proto::Packet {
                                method: b"ERR\0\0\0\0\0".to_owned(),
                                data: e.to_string().as_bytes().to_owned()
                            };

                            response.write(&mut stream).await?;

                            Err(e)
                        }
                    }
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
    type Input<'a> = String;
    type Output = Vec<u8>;

    const NAME: [u8; 8] = *b"GET\0\0\0\0\0";

    fn call<'a>(files: &Files, name: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("retrieving file {name}");
        let object = files.lookup(&name)?
            .ok_or::<io::Error>(io::ErrorKind::NotFound.into())?;
        log::info!("got object {}; returning", object.hex());

        let data = files.get(&object)?;

        Ok((&data[..]).to_owned())
    }
}

struct Insert;

impl Method for Insert {
    type Input<'a> = (String, Vec<u8>);
    type Output = ();

    const NAME: [u8; 8] = *b"INSERT\0\0";

    fn call<'a>(files: &Files, (name, data): Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("storing file {name}");
        log::info!("file contents: {:?}", String::from_utf8_lossy(&data));
        let object = files.insert(&name, &data)?;
        log::info!("stored {name} (object {})", object.hex());

        Ok(())
    }
}