use std::path::PathBuf;
use std::net::SocketAddr;

use serde::Deserialize;

/// Configuration for the client
#[derive(Deserialize)]
pub struct Config {
    /// Client configuration
    pub client: Client,
    /// List of sync points
    pub sync: Vec<Sync>
}

/// An individual sync point, used to sync to/from the server
#[derive(Deserialize)]
pub struct Sync {
    pub to: String,
    pub from: PathBuf
}

    /// Client configuration
#[derive(Deserialize)]
pub struct Client {
    /// The remote server to connect to
    pub remote: SocketAddr
}