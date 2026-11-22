//! The TLS listener and authenticated ping/pong server.
//!
//! One tokio task per connection. Live tasks are bounded by a semaphore
//! acquired *before* `accept`, so at capacity the kernel backlog absorbs the
//! burst rather than us queueing sockets in userspace.
//!
//! A connection is not usable until mutual Ed25519 authentication completes.
//! `Request`/`Response` frames are only accepted in the `Ready` phase.

use std::{
    collections::HashSet,
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::Args;
use rustls::ServerConfig;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use crate::{
    auth::{self, Handshake, Role},
    identity::{self, Identity},
    method::{Context as MethodContext, Registry},
    proto::{
        ALPN, ErrorCode, Features, HelloAck, Id, MAX_AUTH, MAX_FRAME, MAX_HELLO, Message,
        PROTOCOL_VERSION, Response, ResponseBody, SigAlg, WireError,
    },
    tls,
};

/// How long a peer gets to complete the TLS handshake before we drop it.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Total budget for the application handshake (Hello through AuthOk).
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an authenticated connection may sit idle.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Args, Debug, Clone)]
pub struct ServerArgs {
    /// Server TOML config. CLI flags override values in this file.
    #[arg(long, env = "KSYNC_SERVER_CONFIG")]
    config: Option<PathBuf>,

    /// Address to listen on. Port 0 picks an ephemeral port and prints it.
    #[arg(long, env = "KSYNC_ADDR")]
    addr: Option<SocketAddr>,

    /// Data directory (app identity lives here; the store will too).
    #[arg(long, env = "KSYNC_DATA")]
    dir: Option<PathBuf>,

    /// Ed25519 identity seed. Defaults to <data dir>/identity.key.
    #[arg(long, env = "KSYNC_SERVER_IDENTITY_KEY")]
    identity_key: Option<PathBuf>,

    /// PEM certificate chain. Requires --tls-key.
    #[arg(long, env = "KSYNC_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// PEM PKCS#8 private key. Requires --tls-cert.
    #[arg(long, env = "KSYNC_TLS_KEY")]
    tls_key: Option<PathBuf>,

    /// Generate and use a private development CA and a localhost leaf.
    ///
    /// The client must trust the printed CA; verification stays on.
    #[arg(long)]
    dev: bool,

    /// File of allowed client identity ids, one hex id per line.
    #[arg(long, env = "KSYNC_AUTHORIZED_CLIENTS")]
    authorized_clients: Option<PathBuf>,

    /// Maximum number of simultaneously served connections.
    #[arg(long)]
    max_conn: Option<usize>,
}

/// Process-wide state shared by every connection.
pub struct Server {
    identity: Identity,
    allowed_clients: HashSet<Id>,
    registry: Arc<Registry>,
}

impl Server {
    pub fn new(identity: Identity, allowed_clients: HashSet<Id>, registry: Arc<Registry>) -> Self {
        Self {
            identity,
            allowed_clients,
            registry,
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}

pub async fn run(args: ServerArgs) -> Result<()> {
    let settings = crate::config::ServerSettings::resolve(
        args.config.as_deref(),
        crate::config::ServerOverrides {
            addr: args.addr,
            data_dir: args.dir,
            identity_key: args.identity_key,
            tls_cert: args.tls_cert,
            tls_key: args.tls_key,
            dev: args.dev,
            authorized_clients: args.authorized_clients,
            max_conn: args.max_conn,
        },
    )?;

    let tls_config = build_tls_config(&settings)?;

    let identity_path = &settings.identity_key;
    let identity = Identity::load_or_create(identity_path)?;
    println!("  server id       {:?}", identity.id());
    println!("  identity key    {}", identity_path.display());

    let clients_path = &settings.authorized_clients;
    let allowed_clients = identity::load_allowed_clients(clients_path)?;
    println!(
        "  allowed clients {} ({})",
        allowed_clients.len(),
        clients_path.display()
    );

    let server = Arc::new(Server::new(
        identity,
        allowed_clients,
        Arc::new(crate::methods::core_registry()?),
    ));

    let listener = TcpListener::bind(settings.addr)
        .await
        .with_context(|| format!("cannot bind {}", settings.addr))?;
    let local = listener.local_addr()?;

    println!("listening on {local}");
    println!("  max connections {}", settings.max_conn);

    serve(
        listener,
        TlsAcceptor::from(tls_config),
        server,
        settings.max_conn,
    )
    .await
}

fn build_tls_config(settings: &crate::config::ServerSettings) -> Result<Arc<ServerConfig>> {
    if settings.dev {
        let (identity, paths) = tls::dev_identity(&settings.data_dir)?;
        println!("  identity        {} (development)", paths.cert.display());
        println!("  trust this CA   {}", paths.ca_cert.display());
        println!("  CA fingerprint  sha256:{}", fingerprint(&paths.ca_cert)?);
        return tls::server_config(identity);
    }

    let cert = settings
        .tls_cert
        .as_ref()
        .context("no tls_cert configured")?;
    let key = settings.tls_key.as_ref().context("no tls_key configured")?;
    println!("  identity        {}", cert.display());
    tls::server_config(tls::load_identity(cert, key)?)
}

fn fingerprint(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let certs = tls::load_certificates(path)?;
    let digest = Sha256::digest(certs[0].as_ref());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Accept connections forever, bounded by `max_conn` live tasks.
///
/// Public so integration tests can bind an ephemeral port and drive the same
/// loop the binary uses.
pub async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    server: Arc<Server>,
    max_conn: usize,
) -> Result<()> {
    let permits = Arc::new(Semaphore::new(max_conn.max(1)));

    loop {
        // Acquire *before* accepting. When at capacity we stop pulling
        // connections off the queue entirely, so the kernel backlog does the
        // queueing instead of us holding sockets open.
        let permit = Arc::clone(&permits)
            .acquire_owned()
            .await
            .context("connection limiter closed")?;

        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            // A single failed accept (transient resource pressure) must not
            // take the whole server down.
            Err(err) => {
                eprintln!("accept failed: {err}");
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            // The permit is released on drop, including if the handler
            // panics.
            let _permit = permit;

            if let Err(err) = handle(stream, peer, acceptor, server).await {
                eprintln!("{peer}: {err:#}");
            }
        });
    }
}

async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    server: Arc<Server>,
) -> Result<()> {
    stream.set_nodelay(true)?;

    let started = Instant::now();
    let mut tls = timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream))
        .await
        .context("TLS handshake timed out")??;

    println!(
        "{peer}: {} (handshake {:.1?})",
        tls::describe(tls.get_ref().1),
        started.elapsed(),
    );

    if tls.get_ref().1.alpn_protocol() != Some(ALPN) {
        bail!("peer did not negotiate the ksync ALPN protocol");
    }

    let client = timeout(AUTH_TIMEOUT, authenticate(&mut tls, &server))
        .await
        .context("authentication timed out")??;
    println!("{peer}: authenticated as {client:?}");

    let result = request_loop(&mut tls, peer, client, &server).await;

    // Best-effort close_notify so the peer sees a clean shutdown rather than
    // a truncated stream.
    let _ = tls.shutdown().await;

    let requests = result?;
    println!("{peer}: closed after {requests} request(s)");
    Ok(())
}

async fn authenticate(tls: &mut TlsStream<TcpStream>, server: &Server) -> Result<Id> {
    let hello = match crate::proto::read_typed(tls, MAX_HELLO).await? {
        Message::Hello(hello) => hello,
        other => {
            send_error(tls, ErrorCode::Protocol, "expected Hello").await;
            bail!("expected Hello, got {other:?}");
        }
    };

    let version = match crate::proto::negotiate(hello.version_min, hello.version_max) {
        Some(version) => version,
        None => {
            send_error(
                tls,
                ErrorCode::VersionMismatch,
                format!("supported version is {PROTOCOL_VERSION:?}"),
            )
            .await;
            bail!(
                "no version overlap: client offered {:?}..={:?}",
                hello.version_min,
                hello.version_max
            );
        }
    };

    if hello.sig_alg != SigAlg::Ed25519 {
        send_error(tls, ErrorCode::Protocol, "unsupported signature algorithm").await;
        bail!("unsupported signature algorithm {:?}", hello.sig_alg);
    }

    // All auth failures look the same to the peer: no key enumeration.
    if hello.acceptor_id != server.identity.id()
        || identity::verify_identity(hello.sig_alg, &hello.initiator_pub, hello.initiator_id)
            .is_err()
        || !server.allowed_clients.contains(&hello.initiator_id)
    {
        send_error(tls, ErrorCode::AuthFailed, "").await;
        bail!("authentication failed");
    }

    let features = hello.features.intersection(Features::NONE);
    let acceptor_nonce = identity::random_nonce()?;
    let exporter = auth::server_exporter(tls.get_ref().1, version)?;

    let handshake = Handshake {
        version,
        features,
        sig_alg: hello.sig_alg,
        initiator_nonce: hello.initiator_nonce,
        acceptor_nonce,
        initiator_id: hello.initiator_id,
        acceptor_id: server.identity.id(),
        initiator_pub: hello.initiator_pub,
        acceptor_pub: server.identity.public(),
        exporter,
    };

    crate::proto::write_typed(
        tls,
        &Message::HelloAck(HelloAck {
            version,
            sig_alg: handshake.sig_alg,
            features,
            acceptor_nonce,
            acceptor_id: handshake.acceptor_id,
            acceptor_pub: handshake.acceptor_pub,
            initiator_id: handshake.initiator_id,
        }),
    )
    .await?;

    let auth = match crate::proto::read_typed(tls, MAX_AUTH).await? {
        Message::Auth(auth) => auth,
        other => {
            send_error(tls, ErrorCode::Protocol, "expected Auth").await;
            bail!("expected Auth, got {other:?}");
        }
    };

    if handshake.verify(Role::Initiator, &auth.signature).is_err() {
        send_error(tls, ErrorCode::AuthFailed, "").await;
        bail!("authentication failed");
    }

    let signature = handshake.sign(&server.identity, Role::Acceptor)?;
    crate::proto::write_typed(
        tls,
        &Message::AuthOk(crate::proto::AuthOk {
            sig_alg: handshake.sig_alg,
            signature,
        }),
    )
    .await?;

    Ok(handshake.initiator_id)
}

async fn request_loop(
    tls: &mut TlsStream<TcpStream>,
    peer: SocketAddr,
    client: Id,
    server: &Server,
) -> Result<u64> {
    let mut requests = 0;

    loop {
        let message = match timeout(IDLE_TIMEOUT, crate::proto::read_typed(tls, MAX_FRAME)).await {
            Err(_) => {
                eprintln!("{peer}: idle timeout");
                break;
            }
            Ok(Err(err)) if is_clean_eof(&err) => break,
            Ok(Err(err)) => return Err(err),
            Ok(Ok(message)) => message,
        };

        match message {
            Message::Request(request) => {
                requests += 1;

                let mut ctx = MethodContext {
                    peer: client,
                    request_id: request.id,
                };

                let body = match server
                    .registry
                    .call(&mut ctx, &request.method, &request.payload)
                    .await
                {
                    Ok(payload) => ResponseBody::Ok(payload),
                    Err(err) => ResponseBody::Error(err.into_wire()),
                };

                crate::proto::write_typed(
                    tls,
                    &Message::Response(Response {
                        id: request.id,
                        body,
                    }),
                )
                .await?;
            }

            Message::Error(err) => {
                eprintln!(
                    "{peer}: client sent error {:?}: {}",
                    err.code,
                    err.message()
                );
                break;
            }

            other => {
                send_error(tls, ErrorCode::Protocol, "unexpected message").await;
                bail!("unexpected message in Ready: {other:?}");
            }
        }
    }

    Ok(requests)
}

async fn send_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    code: ErrorCode,
    detail: impl Into<String>,
) {
    let _ = crate::proto::write_typed(writer, &Message::Error(WireError::new(code, detail))).await;
}

/// `read_exact` reports a clean peer close as `UnexpectedEof`; that is normal
/// shutdown, not a protocol failure.
fn is_clean_eof(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|err| err.kind() == io::ErrorKind::UnexpectedEof)
    })
}
