//! A malicious app-level relay must fail authentication.
//!
//! The relay terminates TLS separately with the client and the server,
//! forwarding the application handshake verbatim. The signatures are valid in
//! isolation, but the TLS exporter differs between the two sessions, so the
//! server must reject the relayed `Auth`.

use std::{collections::HashSet, fs, path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use ksync::{
    client,
    config::ServerProfile,
    identity::Identity,
    server::{self, Server},
    tls,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ksync-mitm-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
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

struct RealServer {
    addr: std::net::SocketAddr,
    server_id: ksync::proto::Id,
    ca: std::path::PathBuf,
    shutdown: watch::Sender<bool>,
    handle: JoinHandle<Result<()>>,
}

impl RealServer {
    fn stop(self) {
        let _ = self.shutdown.send(true);
        self.handle.abort();
    }
}

async fn start_real_server(scratch: &Scratch, allowed_client: ksync::proto::Id) -> RealServer {
    let (tls_identity, paths) = tls::dev_identity(scratch.path()).unwrap();
    let tls_config = tls::server_config(tls_identity).unwrap();

    let identity =
        Identity::load_or_create(&scratch.path().join(ksync::identity::IDENTITY_FILE)).unwrap();
    let server_id = identity.id();

    let mut allowed = HashSet::new();
    allowed.insert(allowed_client);
    let app = Arc::new(Server::new(
        identity,
        allowed,
        Arc::new(ksync::methods::core_registry().unwrap()),
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, shutdown_rx) = ksync::shutdown::channel();

    let handle = tokio::spawn(server::serve(
        listener,
        TlsAcceptor::from(tls_config),
        app,
        8,
        shutdown_rx,
    ));

    RealServer {
        addr,
        server_id,
        ca: paths.ca_cert,
        shutdown,
        handle,
    }
}

#[tokio::test]
async fn a_relayed_handshake_is_rejected() {
    let scratch = Scratch::new("relay");
    let victim = Identity::generate().unwrap();

    let real = start_real_server(&scratch, victim.id()).await;

    // The attacker has its own CA, trusted only by the victim in this test.
    let attacker_dir = scratch.path().join("attacker");
    let (attacker_tls, attacker_paths) = tls::dev_identity(&attacker_dir).unwrap();
    let attacker_acceptor = TlsAcceptor::from(tls::server_config(attacker_tls).unwrap());

    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();

    let real_ca = real.ca.clone();
    let real_addr = real.addr;

    let relay = tokio::spawn(async move {
        let (client_tcp, _) = relay_listener.accept().await?;
        let mut client_tls = attacker_acceptor.accept(client_tcp).await?;

        let server_tcp = TcpStream::connect(real_addr).await?;
        let client_config = tls::client_config(Some(&real_ca))?;
        let server_name = rustls::pki_types::ServerName::try_from("localhost")?;
        let mut server_tls = TlsConnector::from(client_config)
            .connect(server_name, server_tcp)
            .await?;

        // Forward complete frames verbatim. The relay understands framing but
        // not the signatures; the exporter binding is what defeats it.
        loop {
            let body = match ksync::proto::read_frame(&mut client_tls).await {
                Ok(body) => body,
                Err(_) => break,
            };
            ksync::proto::write_frame(&mut server_tls, &body).await?;

            let body = match ksync::proto::read_frame(&mut server_tls).await {
                Ok(body) => body,
                Err(_) => break,
            };
            ksync::proto::write_frame(&mut client_tls, &body).await?;
        }

        Ok::<(), anyhow::Error>(())
    });

    let profile = ServerProfile {
        name: "real".into(),
        addr: relay_addr,
        server_name: "localhost".into(),
        server_id: real.server_id,
        ca: Some(attacker_paths.ca_cert),
    };

    let result = client::connect(&profile, &victim).await;
    let error = match result {
        Ok(_) => panic!("relayed handshake must not authenticate"),
        Err(error) => format!("{error:#}"),
    };

    assert!(
        error.contains("AuthFailed") || error.contains("authentication"),
        "expected an auth failure, got: {error}"
    );

    let _ = timeout(Duration::from_secs(5), relay).await;
    real.stop();
}
