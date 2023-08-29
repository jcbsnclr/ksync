use clap::Parser;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::client::Client;
use crate::files::crypto;
use crate::files::Path;
use crate::server::methods;

#[derive(Parser)]
pub enum Method {
    Get {
        #[arg(short, long)]
        from: String,

        #[arg(short, long)]
        to: Option<PathBuf>,
    },

    Insert {
        #[arg(short, long)]
        to: String,

        #[arg(short, long)]
        from: Option<PathBuf>,
    },

    Configure {
        #[arg(short, long)]
        admin_path: PathBuf,
        #[arg(short, long)]
        server_path: PathBuf,
        #[arg(short, long)]
        client_path: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("no client key provided")]
    NoKey,
    #[error("no remote to connect to provided")]
    NoRemote,

    #[error("failed to open key '{path:?}': {error}")]
    FailedReadKey { path: PathBuf, error: anyhow::Error },

    #[error("connection to remote '{addr}' failed: {error}")]
    ConnectionFailed { addr: SocketAddr, error: io::Error },

    #[error("command failed: {error}")]
    CommandFailed { error: anyhow::Error },

    #[error("failed to authenticate with server")]
    AuthenticationFailed,
}

impl CliError {
    fn command_failed(error: impl Into<anyhow::Error>) -> CliError {
        CliError::CommandFailed {
            error: error.into(),
        }
    }
}

pub async fn invoke(
    key: Option<PathBuf>,
    remote: Option<SocketAddr>,
    method: Method,
) -> Result<(), CliError> {
    let key = key.ok_or(CliError::NoKey)?;
    let remote = remote.ok_or(CliError::NoRemote)?;

    let mut client = Client::connect(remote)
        .await
        .map_err(|e| CliError::ConnectionFailed {
            addr: remote,
            error: e,
        })?;

    let key_data = tokio::fs::read(&key)
        .await
        .map_err(|e| CliError::FailedReadKey {
            path: key.clone(),
            error: e.into(),
        })?;

    let key: crypto::Key =
        bincode::deserialize(&key_data).map_err(|e| CliError::FailedReadKey {
            path: key,
            error: e.into(),
        })?;

    if !matches!(method, Method::Configure { .. }) {
        client
            .invoke(methods::auth::Identify, key)
            .await
            .map_err(|_| CliError::AuthenticationFailed)?;
    }

    match method {
        Method::Get { from, to } => {
            // parse from string as a server Path
            let from = Path::new(&from).map_err(CliError::command_failed)?;

            // get the file's contents from the server
            let data = client
                .invoke(methods::fs::Get, from)
                .await
                .map_err(CliError::command_failed)?;

            if let Some(to) = to {
                // write response to file
                tokio::fs::write(&to, &data)
                    .await
                    .map_err(CliError::command_failed)?;
            } else {
                // write response to stdout
                let mut stdout = tokio::io::stdout();

                stdout.write(&data).await.unwrap();
            }
        }

        Method::Insert { to, from } => {
            let data = if let Some(from) = from {
                // read data from file
                tokio::fs::read(from)
                    .await
                    .map_err(CliError::command_failed)?
            } else {
                // read data from stdin
                let mut stdin = tokio::io::stdin();
                let mut buf = vec![];

                stdin.read_to_end(&mut buf).await.unwrap();

                buf
            };

            // parse to string as path
            let to = Path::new(&to).map_err(CliError::command_failed)?;

            // insert data to server
            client
                .invoke(methods::fs::Insert, (to, data))
                .await
                .map_err(CliError::command_failed)?;
        }

        Method::Configure {
            admin_path,
            server_path,
            client_path,
        } => {
            let admin_data = tokio::fs::read(&admin_path).await.unwrap();
            let server_data = tokio::fs::read(&server_path).await.unwrap();
            let client_data = tokio::fs::read(&client_path).await.unwrap();

            let keys: [crypto::Key; 3] = [
                bincode::deserialize(&admin_data).unwrap(),
                bincode::deserialize(&server_data).unwrap(),
                bincode::deserialize(&client_data).unwrap(),
            ];

            client
                .invoke(methods::admin::Configure, keys)
                .await
                .map_err(CliError::command_failed)?;
        }
    }

    Ok(())
}
