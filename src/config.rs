use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// Config format for server
#[derive(Deserialize, Debug)]
pub struct Config {
    /// Server configuration
    pub server: Option<Server>,
    pub sync: Option<Sync>,
    pub client: Option<Client>,
}

/// Server configuration
#[derive(Deserialize, Debug)]
pub struct Server {
    /// The address to bind to
    pub addr: SocketAddr,
    /// Location of files database
    pub db: PathBuf,
}

#[derive(Deserialize, Debug)]
pub struct Sync {
    pub remote: SocketAddr,
    pub point: SyncPoint,
    pub resync_time: u64,
    pub key: PathBuf,
}

#[derive(Deserialize, Debug)]
pub struct SyncPoint {
    pub dir: PathBuf,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Client {
    pub remote: SocketAddr,
    pub key: PathBuf,
}
