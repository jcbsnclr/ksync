use tokio::net;

use serde::{Serialize, Deserialize};

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{SystemTime, Duration};

use crate::files::{Files, Object, Revision};
use crate::config;
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

impl Server {
    // initialise the server with a given configuration
    pub async fn init(config: config::Server) -> anyhow::Result<Server> {
        ServerBuilder::new()
            // install method handlers we need
            .add(Get)
            .add(Insert)
            .add(GetListing)
            .add(Clear)
            .add(Delete)
            .add(Rollback)
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
            if let Some(&mut object) = node.traverse(path)?.and_then(|n| n.file()) {
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

/// The [Insert] methods creates an object for a given piece of data, and inserts it into the filesystem at a given path
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

pub struct Delete;

impl Method for Delete {
    type Input<'a> = Path<'a>;
    type Output = ();

    const NAME: &'static str = "DELETE";

    fn call<'a>(files: &Files, path: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("deleting file {path}");

        files.with_root("root", |node| {
            node.delete(path)?;

            Ok(())
        })?;

        Ok(())
    }
}

/// A list of files stored on the server, with their path, object, and timestamp
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileListing(Vec<(String, Option<Object>, u128)>);

impl FileListing {
    pub fn iter(&self) -> impl Iterator<Item = &(String, Option<Object>, u128)> {
        self.0.iter()
    }

    pub fn as_map<'a>(&'a self) -> HashMap<Path<'a>, (Option<Object>, SystemTime)> {
        let files = self
            .iter()
            .map(|(p,o,t)| (Path::new(p).unwrap(), (o.clone(), SystemTime::UNIX_EPOCH + Duration::from_nanos(*t as u64))));

        HashMap::from_iter(files)
    }
}

/// Retrieves a [FileListing] from the server
pub struct GetListing;

impl Method for GetListing {
    type Input<'a> = ();
    type Output = FileListing;
    // type Output = ();

    const NAME: &'static str = "GET_TREE";

    fn call<'a>(files: &Files, _: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("retrieving file listing");

        let output = files.with_root("root", |node| {
            Ok(FileListing(node.file_list()?.collect()))
        })?;

        Ok(output)
    }
}

/// Clear the files database
pub struct Clear;

impl Method for Clear {
    type Input<'a> = ();
    type Output = ();

    const NAME: &'static str = "CLEAR";

    fn call<'a>(files: &Files, _: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("clearing database");
        files.clear()?;

        Ok(())
    }
}

pub struct Rollback;

impl Method for Rollback {
    type Input<'a> = Revision;
    type Output = ();

    const NAME: &'static str = "ROLLBACK";

    fn call<'a>(files: &Files, revision: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let mut old_root = files.get_root("root", revision)?;
        let new_root = files.get_root("root", Revision::FromLatest(0))?;

        old_root.merge(new_root)?;

        files.set_root("root", old_root)?;

        Ok(())
    }
}