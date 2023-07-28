use std::path::PathBuf;
use std::net::SocketAddr;

use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub server: Server
}

#[derive(Deserialize, Debug)]
pub struct Server {
    pub addr: SocketAddr,
    pub db: PathBuf
}