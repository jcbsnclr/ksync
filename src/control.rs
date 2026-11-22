//! Local daemon control protocol.
//!
//! Same postcard codec and length-prefixed framing as the network protocol,
//! but a separate schema. This is a local, same-user control plane, not a
//! network service.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::net::UnixStream;

use crate::proto::{read_frame, write_frame};

pub const CONTROL_VERSION: u16 = 1;

#[derive(Debug, Serialize, serde::Deserialize)]
pub struct ControlRequest {
    pub version: u16,
    pub command: ControlCommand,
}

#[derive(Debug, Serialize, serde::Deserialize)]
pub enum ControlCommand {
    Status,
    Ping { server: Option<String> },
    Sync { server: Option<String> },
}

#[derive(Debug, Serialize, serde::Deserialize)]
pub struct ControlResponse {
    pub version: u16,
    pub result: ControlResult,
}

#[derive(Debug, Serialize, serde::Deserialize)]
pub enum ControlResult {
    Status(DaemonStatus),
    Pong { server: String, rtt_micros: u64 },
    Ok,
    Error { code: u16, detail: String },
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub servers: Vec<ServerStatus>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ServerStatus {
    pub name: String,
    pub connected: bool,
    pub last_error: Option<String>,
}

pub mod error_code {
    pub const BAD_VERSION: u16 = 0;
    pub const UNKNOWN_SERVER: u16 = 1;
    pub const NOT_CONNECTED: u16 = 2;
    pub const TIMEOUT: u16 = 3;
    pub const NOT_IMPLEMENTED: u16 = 4;
    pub const INTERNAL: u16 = 5;
}

pub async fn send<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let body = postcard::to_stdvec(value)?;
    write_frame(stream, &body).await
}

pub async fn recv<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let body = read_frame(stream).await?;
    let (value, trailing) = postcard::take_from_bytes(&body)?;
    if !trailing.is_empty() {
        bail!("trailing bytes in control frame");
    }
    Ok(value)
}

/// Send one control command and return the result.
pub async fn request(
    config: &crate::config::ClientConfig,
    command: ControlCommand,
) -> Result<ControlResult> {
    let mut stream = UnixStream::connect(&config.control_socket)
        .await
        .with_context(|| {
            format!(
                "daemon is not running (control socket {})",
                config.control_socket.display()
            )
        })?;

    send(
        &mut stream,
        &ControlRequest {
            version: CONTROL_VERSION,
            command,
        },
    )
    .await?;

    let response: ControlResponse = recv(&mut stream).await?;
    if response.version != CONTROL_VERSION {
        bail!(
            "daemon control version {} does not match client version {}",
            response.version,
            CONTROL_VERSION
        );
    }

    Ok(response.result)
}

/// Remove a control socket if no daemon is listening on it.
pub async fn clear_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match UnixStream::connect(path).await {
        Ok(_) => bail!("daemon is already running on {}", path.display()),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot remove stale socket {}", path.display()))
        }
        Err(err) => Err(err).with_context(|| {
            format!(
                "cannot determine whether the daemon at {} is alive",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_round_trip() {
        let request = ControlRequest {
            version: CONTROL_VERSION,
            command: ControlCommand::Ping {
                server: Some("home".into()),
            },
        };
        let encoded = postcard::to_stdvec(&request).unwrap();
        let decoded: ControlRequest = postcard::from_bytes(&encoded).unwrap();
        assert!(matches!(
            decoded.command,
            ControlCommand::Ping { server: Some(ref s) } if s == "home"
        ));

        let response = ControlResponse {
            version: CONTROL_VERSION,
            result: ControlResult::Pong {
                server: "home".into(),
                rtt_micros: 42,
            },
        };
        let encoded = postcard::to_stdvec(&response).unwrap();
        let decoded: ControlResponse = postcard::from_bytes(&encoded).unwrap();
        assert!(matches!(
            decoded.result,
            ControlResult::Pong { rtt_micros: 42, .. }
        ));
    }
}
