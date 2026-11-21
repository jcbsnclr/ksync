//! End-to-end protocol tests.
//!
//! These drive the same `server::serve` loop the binary uses, over an
//! ephemeral port, with a real rustls client. TLS verification stays on: the
//! development CA is added as a trust anchor, never bypassed. Application
//! authentication is mutual Ed25519 and is required before any request.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use ksync::{
    client,
    identity::Identity,
    proto::{Features, Id, RequestBody, RequestId, ResponseBody},
    server::{self, Server},
    tls,
};
use rustls::{ClientConfig, ProtocolVersion, RootCertStore, pki_types::ServerName};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, client::TlsStream};

/// Temporary data directory, removed even if an assertion panics.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ksync-handshake-{}-{}-{tag}",
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

struct TestServer {
    addr: std::net::SocketAddr,
    paths: tls::DevPaths,
    server_id: Id,
    handle: JoinHandle<Result<()>>,
}

impl TestServer {
    fn stop(self) {
        self.handle.abort();
    }
}

async fn start_server(scratch: &Scratch, allowed_client: Id) -> TestServer {
    let (tls_identity, paths) = tls::dev_identity(scratch.path()).unwrap();
    let tls_config = tls::server_config(tls_identity).unwrap();

    let identity =
        Identity::load_or_create(&scratch.path().join(ksync::identity::IDENTITY_FILE)).unwrap();
    let server_id = identity.id();

    let mut allowed = HashSet::new();
    allowed.insert(allowed_client);
    let app = Arc::new(Server::new(identity, allowed));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(server::serve(
        listener,
        TlsAcceptor::from(tls_config),
        app,
        8,
    ));

    TestServer {
        addr,
        paths,
        server_id,
        handle,
    }
}

async fn connect_tls(
    addr: std::net::SocketAddr,
    config: Arc<ClientConfig>,
    server_name: &str,
) -> Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from(server_name.to_string())?;
    Ok(TlsConnector::from(config).connect(name, tcp).await?)
}

async fn connect_authenticated(
    server: &TestServer,
    identity: &Identity,
) -> Result<TlsStream<TcpStream>> {
    let config = tls::client_config(Some(&server.paths.ca_cert))?;
    let tls = connect_tls(server.addr, config, "localhost").await?;
    client::handshake(tls, identity, server.server_id, Features::NONE).await
}

#[tokio::test]
async fn authenticates_and_answers_ping() {
    let scratch = Scratch::new("ping");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch, client_identity.id()).await;

    let mut tls = connect_authenticated(&server, &client_identity)
        .await
        .expect("authentication should succeed");

    {
        let conn = tls.get_ref().1;
        assert_eq!(conn.protocol_version(), Some(ProtocolVersion::TLSv1_3));
        assert_eq!(
            conn.negotiated_key_exchange_group()
                .map(|group| group.name()),
            Some(rustls::NamedGroup::X25519MLKEM768)
        );
    }

    let response = client::request(&mut tls, RequestId(1), RequestBody::Ping)
        .await
        .unwrap();

    assert!(matches!(response, ResponseBody::Pong));

    server.stop();
}

#[tokio::test]
async fn an_untrusted_ca_is_rejected() {
    let scratch = Scratch::new("untrusted");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch, client_identity.id()).await;

    // WebPKI roots only: the development CA is not trusted here.
    let config = tls::client_config(None).unwrap();
    let result = connect_tls(server.addr, config, "localhost").await;

    assert!(result.is_err(), "untrusted dev CA must not verify");

    server.stop();
}

#[tokio::test]
async fn a_hostname_mismatch_is_rejected() {
    let scratch = Scratch::new("name");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch, client_identity.id()).await;

    let config = tls::client_config(Some(&server.paths.ca_cert)).unwrap();
    let result = connect_tls(server.addr, config, "example.com").await;

    assert!(
        result.is_err(),
        "a cert for localhost must not verify for example.com"
    );

    server.stop();
}

#[tokio::test]
async fn a_classical_only_client_cannot_connect() {
    let scratch = Scratch::new("classical");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch, client_identity.id()).await;

    let config = classical_client_config(&server.paths.ca_cert);
    let result = connect_tls(server.addr, config, "localhost").await;

    assert!(
        result.is_err(),
        "a client without ML-KEM must fail rather than downgrade"
    );

    server.stop();
}

#[tokio::test]
async fn an_unauthorized_client_is_rejected() {
    let scratch = Scratch::new("unauthorized");
    let allowed = Identity::generate().unwrap();
    let intruder = Identity::generate().unwrap();
    let server = start_server(&scratch, allowed.id()).await;

    let result = connect_authenticated(&server, &intruder).await;

    assert!(
        result.is_err(),
        "a client missing from the allowlist must not authenticate"
    );

    server.stop();
}

#[tokio::test]
async fn a_wrong_expected_server_id_is_rejected_by_the_client() {
    let scratch = Scratch::new("server-id");
    let client_identity = Identity::generate().unwrap();
    let server = start_server(&scratch, client_identity.id()).await;

    let config = tls::client_config(Some(&server.paths.ca_cert)).unwrap();
    let tls = connect_tls(server.addr, config, "localhost").await.unwrap();

    let wrong_server = Identity::generate().unwrap();
    let result = client::handshake(tls, &client_identity, wrong_server.id(), Features::NONE).await;

    assert!(result.is_err(), "client must reject the wrong server id");

    server.stop();
}

/// A client that trusts the dev CA but offers only classical key exchange.
fn classical_client_config(ca: &Path) -> Arc<ClientConfig> {
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();
    provider.kx_groups = vec![rustls::crypto::aws_lc_rs::kx_group::X25519];

    let mut roots = RootCertStore::empty();
    for cert in tls::load_certificates(ca).unwrap() {
        roots.add(cert).unwrap();
    }

    let mut config = ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();

    config.alpn_protocols = vec![ksync::proto::ALPN.to_vec()];
    Arc::new(config)
}
