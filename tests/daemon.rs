//! Daemon lifecycle, control plane, and reconnect tests.

use std::{
    collections::HashSet,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use ksync::{
    config::ClientConfig,
    control::{self, ControlCommand, ControlResult, ServerStatus},
    daemon,
    identity::Identity,
    server::{self, Server},
    shutdown, tls,
};
use tokio::{
    net::{TcpListener, TcpSocket},
    sync::watch,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};
use tokio_rustls::TlsAcceptor;

const WAIT: Duration = Duration::from_secs(10);

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ksync-daemon-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestServer {
    addr: SocketAddr,
    server_id: ksync::proto::Id,
    ca: PathBuf,
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<Result<()>>,
}

impl TestServer {
    async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = timeout(Duration::from_secs(5), self.handle).await;
    }
}

async fn start_server(
    dir: &Path,
    allowed_client: ksync::proto::Id,
    addr: Option<SocketAddr>,
) -> TestServer {
    let (tls_identity, paths) = tls::dev_identity(dir).unwrap();
    let tls_config = tls::server_config(tls_identity).unwrap();

    let identity = Identity::load_or_create(&dir.join(ksync::identity::IDENTITY_FILE)).unwrap();
    let server_id = identity.id();

    let mut allowed = HashSet::new();
    allowed.insert(allowed_client);
    let app = Arc::new(Server::new(
        identity,
        allowed,
        Arc::new(ksync::methods::core_registry().unwrap()),
    ));

    let listener = match addr {
        Some(addr) => {
            // The daemon reconnect test rebinds the same port.
            let socket = TcpSocket::new_v4().unwrap();
            socket.set_reuseaddr(true).unwrap();
            socket.bind(addr).unwrap();
            socket.listen(1024).unwrap()
        }
        None => TcpListener::bind("127.0.0.1:0").await.unwrap(),
    };
    let addr = listener.local_addr().unwrap();

    let (shutdown, shutdown_rx) = shutdown::channel();
    let handle = tokio::spawn(server::serve(
        listener,
        TlsAcceptor::from(tls_config),
        app,
        8,
        shutdown_rx,
    ));

    TestServer {
        addr,
        server_id,
        ca: paths.ca_cert,
        shutdown,
        handle,
    }
}

fn write_client_config(
    dir: &Path,
    identity: &Identity,
    server_addr: SocketAddr,
    server_id: ksync::proto::Id,
    ca: &Path,
) -> PathBuf {
    let identity_path = dir.join("client.key");
    identity.save(&identity_path).unwrap();

    let control_socket = dir.join("control.sock");
    let config = dir.join("client.toml");

    fs::write(
        &config,
        format!(
            r#"
[client]
identity = "{}"
control_socket = "{}"

[[servers]]
name = "dev"
addr = "{server_addr}"
server_name = "localhost"
server_id = "{}"
ca = "{}"
"#,
            identity_path.display(),
            control_socket.display(),
            hex::encode(server_id.0),
            ca.display()
        ),
    )
    .unwrap();

    config
}

async fn wait_for_status(config: &ClientConfig, predicate: impl Fn(&[ServerStatus]) -> bool) {
    let deadline = Instant::now() + WAIT;

    loop {
        if let Ok(ControlResult::Status(status)) =
            control::request(config, ControlCommand::Status).await
            && predicate(&status.servers)
        {
            return;
        }

        assert!(
            Instant::now() < deadline,
            "daemon did not reach the expected state in time"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_connected(config: &ClientConfig) {
    wait_for_status(config, |servers| {
        servers
            .iter()
            .any(|server| server.name == "dev" && server.connected)
    })
    .await;
}

async fn wait_disconnected(config: &ClientConfig) {
    wait_for_status(config, |servers| {
        servers
            .iter()
            .any(|server| server.name == "dev" && !server.connected)
    })
    .await;
}

#[tokio::test]
async fn daemon_pings_and_shuts_down_cleanly() {
    let scratch = Scratch::new("lifecycle");
    let client_identity = Identity::generate().unwrap();

    let server = start_server(&scratch.path().join("server"), client_identity.id(), None).await;

    let config_path = write_client_config(
        scratch.path(),
        &client_identity,
        server.addr,
        server.server_id,
        &server.ca,
    );
    let config = ClientConfig::load(&config_path).unwrap();

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    let daemon_handle = tokio::spawn(daemon::run_until(config_path, shutdown_rx));

    wait_connected(&config).await;

    match control::request(&config, ControlCommand::Ping { server: None })
        .await
        .unwrap()
    {
        ControlResult::Pong { server, .. } => assert_eq!(server, "dev"),
        other => panic!("unexpected ping result: {other:?}"),
    }

    shutdown_tx.send(true).unwrap();
    timeout(Duration::from_secs(5), daemon_handle)
        .await
        .expect("daemon should shut down")
        .expect("daemon task should not panic")
        .expect("daemon should exit cleanly");

    assert!(
        !config.control_socket.exists(),
        "control socket should be removed on shutdown"
    );

    server.stop().await;
}

#[tokio::test]
async fn daemon_reconnects_after_server_restart() {
    let scratch = Scratch::new("reconnect");
    let client_identity = Identity::generate().unwrap();
    let server_dir = scratch.path().join("server");

    let server = start_server(&server_dir, client_identity.id(), None).await;
    let addr = server.addr;

    let config_path = write_client_config(
        scratch.path(),
        &client_identity,
        addr,
        server.server_id,
        &server.ca,
    );
    let config = ClientConfig::load(&config_path).unwrap();

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    let daemon_handle = tokio::spawn(daemon::run_until(config_path, shutdown_rx));

    wait_connected(&config).await;

    server.stop().await;

    // The daemon notices a dead connection on its next request rather than by
    // polling; this also exercises the failure path.
    let _ = control::request(&config, ControlCommand::Ping { server: None }).await;
    wait_disconnected(&config).await;

    let restarted = start_server(&server_dir, client_identity.id(), Some(addr)).await;
    wait_connected(&config).await;

    shutdown_tx.send(true).unwrap();
    let _ = timeout(Duration::from_secs(5), daemon_handle).await;

    restarted.stop().await;
}

#[tokio::test]
async fn a_second_daemon_refuses_to_start() {
    let scratch = Scratch::new("duplicate");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch.path().join("server"), client_identity.id(), None).await;

    let config_path = write_client_config(
        scratch.path(),
        &client_identity,
        server.addr,
        server.server_id,
        &server.ca,
    );
    let config = ClientConfig::load(&config_path).unwrap();

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    let daemon_handle = tokio::spawn(daemon::run_until(config_path.clone(), shutdown_rx));
    wait_connected(&config).await;

    let (_, second_rx) = shutdown::channel();
    let error = daemon::run_until(config_path, second_rx)
        .await
        .expect_err("second daemon should refuse to start");
    assert!(format!("{error:#}").contains("already running"));

    shutdown_tx.send(true).unwrap();
    let _ = timeout(Duration::from_secs(5), daemon_handle).await;
    server.stop().await;
}
