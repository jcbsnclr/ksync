use std::path::PathBuf;
use std::net::SocketAddr;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub client: Client,
    pub sync: Vec<Sync>
}

#[derive(Deserialize)]
pub struct Sync {
    pub to: String,
    pub from: PathBuf
}

#[derive(Deserialize)]
pub struct Client {
    pub remote: SocketAddr
}