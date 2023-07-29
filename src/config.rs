use std::path::PathBuf;
use std::net::SocketAddr;

use serde::Deserialize;

/// Config format for server
#[derive(Deserialize, Debug)]
pub struct Config {
    /// Server configuration
    pub server: Server
}

/// Server configuration
#[derive(Deserialize, Debug)]
pub struct Server {
    /// The address to bind to
    pub addr: SocketAddr,
    /// Location of files database
    pub db: PathBuf
}