//! TLS plumbing shared by the server and the tests.
//!
//! Two responsibilities live here:
//!
//! 1. [`provider`] builds the rustls crypto backend and pins the key-exchange
//!    list to a single post-quantum hybrid group. Pinning the list to exactly
//!    one entry is the important part: it makes a downgrade to classical
//!    (X25519-only) key exchange impossible. A peer that cannot do ML-KEM
//!    fails the handshake instead of quietly falling back.
//!
//! 2. Identity loading. Production identities are ordinary CA-issued PEM
//!    certificates verified through the normal WebPKI rules; nothing here
//!    pins or special-cases them. [`dev_identity`] is a convenience for local
//!    development: it mints a private CA and a `localhost` leaf, so the client
//!    can trust the CA instead of disabling verification.
//!
//! Client authentication deliberately does not happen at the TLS layer. The
//! certificate only proves which server/domain was reached; proving which
//! *user* is connecting belongs to the application protocol.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};

/// The rustls crypto backend used by every connection in this process.
///
/// Notes on the choices below:
///
/// * `aws_lc_rs` is the only stock rustls provider that ships ML-KEM. `ring`
///   has no post-quantum support, so choosing it would silently remove the
///   property we care about. It does mean we compile some C via
///   `aws-lc-sys`; that is a deliberate trade for a vetted implementation.
/// * `kx_groups` is *replaced*, not reordered. Reordering would still leave
///   the classical groups reachable; replacing makes post-quantum hybrid
///   key exchange mandatory.
/// * The provider is created fresh rather than installed globally via
///   `install_default()`. Every config we build gets this `Arc` explicitly,
///   so nothing depends on process-wide state — which also keeps tests
///   parallel-safe.
pub fn provider() -> Arc<CryptoProvider> {
    let mut provider = rustls::crypto::aws_lc_rs::default_provider();

    // X25519MLKEM768 = ephemeral X25519 + ML-KEM-768, the hybrid group that
    // browsers and TLS libraries negotiated as the post-quantum default.
    provider.kx_groups = vec![rustls::crypto::aws_lc_rs::kx_group::X25519MLKEM768];

    Arc::new(provider)
}

/// A server certificate chain and its private key, already parsed.
pub struct Identity {
    pub certs: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

/// Build a server-side TLS configuration from `identity`.
///
/// The result is `Arc`ed because tokio-rustls clones the `Arc` into every
/// connection, so the config is built once at startup and shared by all
/// tasks.
pub fn server_config(identity: Identity) -> Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::builder_with_provider(provider())
        // TLS 1.3 only. This also means only TLS 1.3 cipher suites are
        // reachable, since the provider's 1.2 suites can no longer be
        // negotiated.
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("this rustls build does not support TLS 1.3")?
        // No client certificates: client identity is application-level.
        .with_no_client_auth()
        .with_single_cert(identity.certs, identity.key)
        .context("server certificate and private key do not match")?;

    // Catch peers that speak something other than ksync before the app
    // handshake. The app-level Hello remains authoritative for versioning.
    config.alpn_protocols = vec![crate::proto::ALPN.to_vec()];

    Ok(Arc::new(config))
}

/// Build a client configuration using normal WebPKI verification.
///
/// `extra_root` adds a private trust anchor (the development CA). That is a
/// root, not a pin: the server can rotate its leaf without the client
/// changing. There is deliberately no way to disable verification here.
pub fn client_config(extra_root: Option<&Path>) -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Some(path) = extra_root {
        for cert in load_certificates(path)? {
            roots
                .add(cert)
                .with_context(|| format!("cannot use {} as a trust anchor", path.display()))?;
        }
    }

    let mut config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("this rustls build does not support TLS 1.3")?
        .with_root_certificates(roots)
        .with_no_client_auth();

    config.alpn_protocols = vec![crate::proto::ALPN.to_vec()];

    Ok(Arc::new(config))
}

pub fn load_identity(cert_path: &Path, key_path: &Path) -> Result<Identity> {
    Ok(Identity {
        certs: load_certificates(cert_path)?,
        key: load_private_key(key_path)?,
    })
}

/// Load all certificates from a PEM file.
pub fn load_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut reader = io::BufReader::new(&data[..]);

    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("cannot parse certificates in {}", path.display()))?;

    if certs.is_empty() {
        bail!("no certificates found in {}", path.display());
    }

    Ok(certs)
}

/// Load the first private key from a PEM file.
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut reader = io::BufReader::new(&data[..]);

    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("cannot parse private key in {}", path.display()))?
        .with_context(|| format!("no private key found in {}", path.display()))
}

/// Paths used by [`dev_identity`], all under `<data dir>/tls`.
#[derive(Debug, Clone)]
pub struct DevPaths {
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub cert: PathBuf,
    pub key: PathBuf,
}

pub fn dev_paths(dir: &Path) -> DevPaths {
    let tls = dir.join("tls");
    DevPaths {
        ca_cert: tls.join("dev-ca.cert.pem"),
        ca_key: tls.join("dev-ca.key.pem"),
        cert: tls.join("server.cert.pem"),
        key: tls.join("server.key.pem"),
    }
}

/// Load or create the development identity under `dir`.
///
/// A private CA is kept separately from the leaf so the leaf can be rotated
/// without changing what the client trusts. The half-exists cases are errors:
/// silently minting a replacement would change the trust anchor or the
/// identity the client expects, and the resulting failure would be far more
/// confusing than a message here.
pub fn dev_identity(dir: &Path) -> Result<(Identity, DevPaths)> {
    let paths = dev_paths(dir);

    let tls_dir = paths
        .ca_cert
        .parent()
        .expect("dev paths always have a parent");
    fs::create_dir_all(tls_dir).with_context(|| format!("cannot create {}", tls_dir.display()))?;

    let ca = load_or_create_ca(&paths)?;

    let identity = match (paths.cert.exists(), paths.key.exists()) {
        (true, true) => load_identity(&paths.cert, &paths.key)?,
        (false, false) => issue_leaf(&paths, ca)?,
        (true, false) => bail!(
            "found {} but {} is missing; refusing to issue a new identity \
             (delete the tls directory to start over)",
            paths.cert.display(),
            paths.key.display()
        ),
        (false, true) => bail!(
            "found {} but {} is missing; refusing to issue a new certificate \
             (delete the tls directory to start over)",
            paths.key.display(),
            paths.cert.display()
        ),
    };

    Ok((identity, paths))
}

struct CaMaterial {
    params: CertificateParams,
    key: KeyPair,
}

fn ca_params() -> Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, "ksync development CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    Ok(params)
}

fn load_or_create_ca(paths: &DevPaths) -> Result<CaMaterial> {
    match (paths.ca_cert.exists(), paths.ca_key.exists()) {
        (true, true) => {
            let key_pem = fs::read_to_string(&paths.ca_key)
                .with_context(|| format!("cannot read {}", paths.ca_key.display()))?;
            let key = KeyPair::from_pem(&key_pem)
                .with_context(|| format!("cannot parse {}", paths.ca_key.display()))?;
            Ok(CaMaterial {
                params: ca_params()?,
                key,
            })
        }

        (false, false) => {
            let params = ca_params()?;
            let key = KeyPair::generate().context("failed to generate development CA key")?;
            let cert = params
                .self_signed(&key)
                .context("failed to generate development CA certificate")?;

            write_public(&paths.ca_cert, cert.pem().as_bytes())?;
            write_private(&paths.ca_key, key.serialize_pem().as_bytes())?;

            Ok(CaMaterial { params, key })
        }

        (true, false) => bail!(
            "found {} but {} is missing; refusing to generate a new CA \
             (delete the tls directory to start over)",
            paths.ca_cert.display(),
            paths.ca_key.display()
        ),
        (false, true) => bail!(
            "found {} but {} is missing; refusing to generate a new CA \
             (delete the tls directory to start over)",
            paths.ca_key.display(),
            paths.ca_cert.display()
        ),
    }
}

fn issue_leaf(paths: &DevPaths, ca: CaMaterial) -> Result<Identity> {
    // SANs cover the names a local test client will use. rustls checks IP
    // literals against IP SANs, not against DNS SANs, so both forms matter.
    let mut params = CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ])?;
    params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key = KeyPair::generate().context("failed to generate development leaf key")?;
    let cert = params
        .signed_by(&key, &Issuer::from_params(&ca.params, &ca.key))
        .context("failed to issue development leaf certificate")?;

    write_public(&paths.cert, cert.pem().as_bytes())?;
    write_private(&paths.key, key.serialize_pem().as_bytes())?;

    Ok(Identity {
        certs: vec![cert.der().clone()],
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    })
}

/// One-line description of what a completed handshake negotiated.
///
/// Shared by the server's log line and, later, the client's, so the two can
/// never drift into describing the same connection differently.
pub fn describe(conn: &rustls::ServerConnection) -> String {
    let version = conn
        .protocol_version()
        .map(|version| format!("{version:?}"))
        .unwrap_or_else(|| "-".into());
    let cipher = conn
        .negotiated_cipher_suite()
        .map(|suite| format!("{:?}", suite.suite()))
        .unwrap_or_else(|| "-".into());
    let group = conn
        .negotiated_key_exchange_group()
        .map(|group| format!("{:?}", group.name()))
        .unwrap_or_else(|| "-".into());

    format!("{version} {cipher} {group}")
}

fn write_public(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;

    file.write_all(bytes)
        .with_context(|| format!("cannot write {}", path.display()))?;

    // `mode` only applies when the file is created. If an existing key was
    // left with wider permissions, tighten it here as well.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_public(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per test so the tests can run in parallel without colliding.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ksync-tls-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn provider_offers_only_the_pq_group() {
        let provider = provider();
        assert_eq!(provider.kx_groups.len(), 1);
        assert_eq!(
            provider.kx_groups[0].name(),
            rustls::NamedGroup::X25519MLKEM768
        );
    }

    #[test]
    fn dev_identity_generates_then_reloads_the_same_leaf() {
        let dir = scratch("reload");

        let (first, paths) = dev_identity(&dir).expect("first run should generate");
        let (second, _) = dev_identity(&dir).expect("second run should load");

        assert_eq!(first.certs, second.certs);
        assert_eq!(
            first.key.secret_der(),
            second.key.secret_der(),
            "reloading must return the same private key"
        );

        assert!(paths.ca_cert.exists());
        assert!(paths.ca_key.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_half_written_leaf_is_rejected() {
        let dir = scratch("half");
        let (_, paths) = dev_identity(&dir).unwrap();

        fs::remove_file(&paths.key).unwrap();

        let err = match dev_identity(&dir) {
            Ok(_) => panic!("expected a half-written identity to be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("missing"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn private_keys_are_not_world_readable() {
        let dir = scratch("mode");
        let (_, paths) = dev_identity(&dir).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for path in [&paths.ca_key, &paths.key] {
                let mode = fs::metadata(path).unwrap().permissions().mode();
                assert_eq!(
                    mode & 0o077,
                    0,
                    "{} should be 0600, got {mode:o}",
                    path.display()
                );
            }
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn server_config_builds_from_a_dev_identity() {
        let dir = scratch("config");
        let (identity, _) = dev_identity(&dir).unwrap();
        let _config = server_config(identity).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
