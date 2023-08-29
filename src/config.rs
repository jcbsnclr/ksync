use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid config: {0}")]
    InvalidConfig(toml::de::Error),

    #[error("config file '{0:?}' not found")]
    NotFound(PathBuf),
}

pub async fn load_config(path: Option<PathBuf>) -> anyhow::Result<Config> {
    let path = if let Some(path) = path {
        path
    } else {
        let config_dir = dirs::config_dir().unwrap();
        config_dir.join("ksync/config.toml")
    };

    if !path.exists() || path.is_dir() {
        Err(Error::NotFound(path).into())
    } else {
        let data = tokio::fs::read_to_string(path).await?;
        let config = toml::from_str(&data).map_err(Error::InvalidConfig)?;

        Ok(config)
    }
}
