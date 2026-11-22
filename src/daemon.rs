//! The per-user sync daemon and its local control socket.
//!
//! One connection task per configured server. The control CLI never sees
//! identities or TLS settings; it asks the daemon to act.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, interval_at, sleep, timeout},
};

use crate::{
    client,
    config::{ClientConfig, ServerProfile},
    control::{
        self, CONTROL_VERSION, ControlCommand, ControlRequest, ControlResponse, ControlResult,
        DaemonStatus, ServerStatus, error_code,
    },
    identity::Identity,
    methods::Ping,
    proto::RequestId,
};

const KEEPALIVE: Duration = Duration::from_secs(90);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

enum ServerCommand {
    Ping {
        reply: oneshot::Sender<std::result::Result<u64, String>>,
    },
}

struct ServerHandle {
    tx: mpsc::Sender<ServerCommand>,
    status: Arc<Mutex<ServerStatus>>,
}

pub async fn run(config_path: PathBuf) -> Result<()> {
    let config = ClientConfig::load(&config_path)?;
    let identity = Arc::new(config.identity()?);

    println!(
        "daemon: identity {:?} ({})",
        identity.id(),
        config.identity.display()
    );

    if let Some(parent) = config.control_socket.parent() {
        let existed = parent.exists();
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        // Only tighten a directory we just created. Never chmod an existing
        // shared directory such as /tmp when the socket path is custom.
        if !existed {
            set_mode(parent, 0o700)?;
        }
    }

    control::clear_stale_socket(&config.control_socket).await?;
    let listener = UnixListener::bind(&config.control_socket)
        .with_context(|| format!("cannot bind {}", config.control_socket.display()))?;
    set_mode(&config.control_socket, 0o600)?;

    let mut handles = HashMap::new();

    for profile in config.servers.clone() {
        let (tx, rx) = mpsc::channel(8);
        let status = Arc::new(Mutex::new(ServerStatus {
            name: profile.name.clone(),
            connected: false,
            last_error: None,
        }));

        tokio::spawn(server_task(
            profile.clone(),
            Arc::clone(&identity),
            Arc::clone(&status),
            rx,
        ));

        handles.insert(profile.name.clone(), ServerHandle { tx, status });
    }

    let handles = Arc::new(handles);
    println!(
        "daemon: listening on {} ({} server(s))",
        config.control_socket.display(),
        handles.len()
    );

    loop {
        let (stream, _) = listener.accept().await?;
        let handles = Arc::clone(&handles);
        tokio::spawn(async move {
            if let Err(err) = handle_control(stream, handles).await {
                eprintln!("control: {err:#}");
            }
        });
    }
}

async fn handle_control(
    mut stream: UnixStream,
    handles: Arc<HashMap<String, ServerHandle>>,
) -> Result<()> {
    let request: ControlRequest = control::recv(&mut stream).await?;

    let result = if request.version != CONTROL_VERSION {
        ControlResult::Error {
            code: error_code::BAD_VERSION,
            detail: format!(
                "control version {} is not supported; daemon speaks {}",
                request.version, CONTROL_VERSION
            ),
        }
    } else {
        match request.command {
            ControlCommand::Status => ControlResult::Status(status(&handles).await),
            ControlCommand::Ping { server } => ping(&handles, server).await,
            ControlCommand::Sync { .. } => ControlResult::Error {
                code: error_code::NOT_IMPLEMENTED,
                detail: "sync is not implemented yet".into(),
            },
        }
    };

    control::send(
        &mut stream,
        &ControlResponse {
            version: CONTROL_VERSION,
            result,
        },
    )
    .await
}

async fn status(handles: &HashMap<String, ServerHandle>) -> DaemonStatus {
    let mut servers = Vec::with_capacity(handles.len());

    for handle in handles.values() {
        servers.push(handle.status.lock().await.clone());
    }

    servers.sort_by(|a, b| a.name.cmp(&b.name));

    DaemonStatus {
        pid: std::process::id(),
        servers,
    }
}

async fn ping(handles: &HashMap<String, ServerHandle>, server: Option<String>) -> ControlResult {
    let handle = match select_server(handles, server) {
        Ok(handle) => handle,
        Err((code, detail)) => return ControlResult::Error { code, detail },
    };

    let (tx, rx) = oneshot::channel();
    if handle
        .tx
        .send(ServerCommand::Ping { reply: tx })
        .await
        .is_err()
    {
        return ControlResult::Error {
            code: error_code::INTERNAL,
            detail: "server task is gone".into(),
        };
    }

    match timeout(COMMAND_TIMEOUT, rx).await {
        Ok(Ok(Ok(rtt_micros))) => ControlResult::Pong {
            server: handle.status.lock().await.name.clone(),
            rtt_micros,
        },
        Ok(Ok(Err(detail))) => ControlResult::Error {
            code: error_code::NOT_CONNECTED,
            detail,
        },
        Ok(Err(_)) => ControlResult::Error {
            code: error_code::INTERNAL,
            detail: "server task dropped the reply".into(),
        },
        Err(_) => ControlResult::Error {
            code: error_code::TIMEOUT,
            detail: "ping timed out".into(),
        },
    }
}

fn select_server(
    handles: &HashMap<String, ServerHandle>,
    server: Option<String>,
) -> std::result::Result<&ServerHandle, (u16, String)> {
    match server {
        Some(name) => handles.get(&name).ok_or_else(|| {
            (
                error_code::UNKNOWN_SERVER,
                format!("unknown server profile {name:?}"),
            )
        }),
        None if handles.len() == 1 => Ok(handles.values().next().unwrap()),
        None => Err((
            error_code::UNKNOWN_SERVER,
            "multiple servers configured; pass a server name".into(),
        )),
    }
}

async fn server_task(
    profile: ServerProfile,
    identity: Arc<Identity>,
    status: Arc<Mutex<ServerStatus>>,
    mut commands: mpsc::Receiver<ServerCommand>,
) {
    let mut backoff = Duration::from_secs(1);

    loop {
        match client::connect(&profile, &identity).await {
            Ok(mut tls) => {
                backoff = Duration::from_secs(1);
                {
                    let mut status = status.lock().await;
                    status.connected = true;
                    status.last_error = None;
                }
                println!("{}: connected to {}", profile.name, profile.addr);

                let mut keepalive = interval_at(Instant::now() + KEEPALIVE, KEEPALIVE);
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                loop {
                    tokio::select! {
                        command = commands.recv() => {
                            match command {
                                Some(ServerCommand::Ping { reply }) => {
                                    let started = Instant::now();
                                    match client::call_core::<Ping>(&mut tls, RequestId(1), ()).await {
                                        Ok(()) => {
                                            let _ = reply.send(Ok(started.elapsed().as_micros() as u64));
                                        }
                                        Err(err) => {
                                            let _ = reply.send(Err(err.to_string()));
                                            break;
                                        }
                                    }
                                }
                                None => return,
                            }
                        }
                        _ = keepalive.tick() => {
                            if let Err(err) = client::call_core::<Ping>(&mut tls, RequestId(1), ()).await {
                                eprintln!("{}: keepalive failed: {err:#}", profile.name);
                                break;
                            }
                        }
                    }
                }

                {
                    let mut status = status.lock().await;
                    status.connected = false;
                    status.last_error = Some("connection lost".into());
                }
                eprintln!("{}: disconnected", profile.name);
            }

            Err(err) => {
                let detail = format!("{err:#}");
                {
                    let mut status = status.lock().await;
                    status.connected = false;
                    status.last_error = Some(detail.clone());
                }
                eprintln!("{}: {detail}", profile.name);
            }
        }

        // During backoff, keep answering control requests instead of letting
        // them sit until the CLI times out.
        let deadline = sleep(backoff);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                _ = &mut deadline => break,
                command = commands.recv() => {
                    match command {
                        Some(ServerCommand::Ping { reply }) => {
                            let _ = reply.send(Err("not connected".into()));
                        }
                        None => return,
                    }
                }
            }
        }

        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &std::path::Path, _mode: u32) -> Result<()> {
    Ok(())
}
