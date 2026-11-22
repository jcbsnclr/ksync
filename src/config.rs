//! Client configuration.
//!
//! TOML, discovered from `$XDG_CONFIG_HOME/ksync/config.toml` (or
//! `$HOME/.config/ksync/config.toml`). Relative paths are resolved against
//! the config file's directory; there is no `~` or environment expansion.

use std::{
    collections::HashSet,
    env, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{identity, proto::Id};

pub const DEFAULT_CONFIG_FILE: &str = "config.toml";
pub const DEFAULT_IDENTITY_FILE: &str = "identity.key";

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub path: PathBuf,
    pub identity: PathBuf,
    pub control_socket: PathBuf,
    pub servers: Vec<ServerProfile>,
}

#[derive(Debug, Clone)]
pub struct ServerProfile {
    pub name: String,
    pub addr: SocketAddr,
    pub server_name: String,
    pub server_id: Id,
    pub ca: Option<PathBuf>,
}

impl ClientConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let raw: RawConfig =
            toml::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let config_dir = base.to_path_buf();

        let identity = match raw.client.identity {
            Some(path) => resolve(base, path),
            None => config_dir.join(DEFAULT_IDENTITY_FILE),
        };

        let control_socket = match raw.client.control_socket {
            Some(path) => resolve(base, path),
            None => default_control_socket(),
        };

        let mut servers = Vec::with_capacity(raw.servers.len());
        let mut names = HashSet::new();

        for entry in raw.servers {
            if entry.name.is_empty() {
                bail!("{}: server name cannot be empty", path.display());
            }
            if !names.insert(entry.name.clone()) {
                bail!("{}: duplicate server name {:?}", path.display(), entry.name);
            }

            let server_id = identity::parse_id(&entry.server_id).with_context(|| {
                format!(
                    "{}: server {:?}: invalid server_id",
                    path.display(),
                    entry.name
                )
            })?;

            if entry.server_name.is_empty() {
                bail!(
                    "{}: server {:?}: server_name cannot be empty",
                    path.display(),
                    entry.name
                );
            }

            servers.push(ServerProfile {
                name: entry.name,
                addr: entry.addr,
                server_name: entry.server_name,
                server_id,
                ca: entry.ca.map(|path| resolve(base, path)),
            });
        }

        if servers.is_empty() {
            bail!("{}: no servers configured", path.display());
        }

        Ok(Self {
            path: path.to_path_buf(),
            identity,
            control_socket,
            servers,
        })
    }

    /// Discover the default config path.
    pub fn discover() -> Result<PathBuf> {
        if let Some(path) = env::var_os("KSYNC_CLIENT_CONFIG") {
            return Ok(PathBuf::from(path));
        }

        let base = config_home()
            .context("cannot determine config directory: set XDG_CONFIG_HOME or HOME")?;
        Ok(base.join("ksync").join(DEFAULT_CONFIG_FILE))
    }

    /// Resolve a server profile, requiring a name when several are configured.
    pub fn server(&self, name: Option<&str>) -> Result<&ServerProfile> {
        match name {
            Some(name) => self
                .servers
                .iter()
                .find(|server| server.name == name)
                .with_context(|| format!("unknown server profile {name:?}")),
            None if self.servers.len() == 1 => Ok(&self.servers[0]),
            None => bail!(
                "multiple servers configured ({}); pass a server name",
                self.servers
                    .iter()
                    .map(|server| server.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Load or create the client identity.
    pub fn identity(&self) -> Result<identity::Identity> {
        identity::Identity::load_or_create(&self.identity)
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    client: RawClient,
    #[serde(default)]
    servers: Vec<RawServer>,
}

#[derive(Debug, Deserialize, Default)]
struct RawClient {
    identity: Option<PathBuf>,
    control_socket: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RawServer {
    name: String,
    addr: SocketAddr,
    server_name: String,
    server_id: String,
    ca: Option<PathBuf>,
}

/// Command-line/environment overrides for the server, merged over TOML.
#[derive(Debug, Default)]
pub struct ServerOverrides {
    pub addr: Option<SocketAddr>,
    pub data_dir: Option<PathBuf>,
    pub identity_key: Option<PathBuf>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub dev: bool,
    pub authorized_clients: Option<PathBuf>,
    pub max_conn: Option<usize>,
}

/// Fully resolved server settings.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub identity_key: PathBuf,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub dev: bool,
    pub authorized_clients: PathBuf,
    pub max_conn: usize,
}

impl ServerSettings {
    /// Merge CLI overrides over an optional server TOML file.
    ///
    /// Precedence: CLI > TOML > defaults. Relative paths in the TOML file
    /// resolve against its directory; paths from the CLI resolve against the
    /// current directory.
    pub fn resolve(path: Option<&Path>, overrides: ServerOverrides) -> Result<Self> {
        let (raw, base) = match path {
            Some(path) => {
                let text = fs::read_to_string(path)
                    .with_context(|| format!("cannot read config {}", path.display()))?;
                let raw: RawServerConfig = toml::from_str(&text)
                    .with_context(|| format!("cannot parse {}", path.display()))?;
                let base = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                (raw.server, base)
            }
            None => (
                RawServerSection::default(),
                std::env::current_dir().context("cannot determine current directory")?,
            ),
        };

        let addr = overrides
            .addr
            .or(raw.addr)
            .unwrap_or_else(|| "127.0.0.1:8443".parse().expect("valid default address"));

        // CLI paths are relative to the invoking directory; TOML paths are
        // relative to the config file.
        let cli_base = std::env::current_dir().context("cannot determine current directory")?;
        let data_dir = match overrides.data_dir {
            Some(path) => resolve(&cli_base, path),
            None => resolve(&base, raw.data_dir.unwrap_or_else(|| PathBuf::from("data"))),
        };
        let identity_key = match overrides.identity_key {
            Some(path) => resolve(&cli_base, path),
            None => match raw.identity_key {
                Some(path) => resolve(&base, path),
                None => data_dir.join(identity::IDENTITY_FILE),
            },
        };
        let tls_cert = match overrides.tls_cert {
            Some(path) => Some(resolve(&cli_base, path)),
            None => raw.tls_cert.map(|path| resolve(&base, path)),
        };
        let tls_key = match overrides.tls_key {
            Some(path) => Some(resolve(&cli_base, path)),
            None => raw.tls_key.map(|path| resolve(&base, path)),
        };
        let dev = overrides.dev || raw.dev.unwrap_or(false);
        let max_conn = overrides.max_conn.or(raw.max_conn).unwrap_or(128);

        let authorized_clients = match overrides.authorized_clients {
            Some(path) => resolve(&cli_base, path),
            None => {
                let path = raw.authorized_clients.context(
                    "no client allowlist: configure authorized_clients or pass \
                     --authorized-clients <FILE>",
                )?;
                resolve(&base, path)
            }
        };

        if max_conn == 0 {
            bail!("max_conn must be at least 1");
        }
        match (dev, tls_cert.is_some(), tls_key.is_some()) {
            (true, true, _) | (true, _, true) => {
                bail!("--dev cannot be combined with tls_cert/tls_key")
            }
            (false, false, false) => {
                bail!("no TLS identity configured: set tls_cert and tls_key, or enable dev")
            }
            (false, true, false) => bail!("tls_cert requires tls_key"),
            (false, false, true) => bail!("tls_key requires tls_cert"),
            _ => {}
        }

        Ok(Self {
            addr,
            data_dir,
            identity_key,
            tls_cert,
            tls_key,
            dev,
            authorized_clients,
            max_conn,
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct RawServerConfig {
    #[serde(default)]
    server: RawServerSection,
}

#[derive(Debug, Deserialize, Default)]
struct RawServerSection {
    addr: Option<SocketAddr>,
    data_dir: Option<PathBuf>,
    identity_key: Option<PathBuf>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    dev: Option<bool>,
    authorized_clients: Option<PathBuf>,
    max_conn: Option<usize>,
}

fn resolve(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

pub fn default_control_socket() -> PathBuf {
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("ksync").join("control.sock");
    }

    // No user runtime dir: use a per-uid directory with restrictive
    // permissions. The socket itself is also created 0600.
    #[cfg(unix)]
    let uid = unsafe { libc::geteuid() };
    #[cfg(not(unix))]
    let uid = 0u32;

    PathBuf::from(format!("/tmp/ksync-{uid}/control.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(tag: &str, body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "ksync-config-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(DEFAULT_CONFIG_FILE);
        fs::write(&path, body).unwrap();
        (dir, path)
    }

    fn id_hex() -> String {
        "11".repeat(32)
    }

    #[test]
    fn loads_a_single_server_and_resolves_paths() {
        let (dir, path) = write(
            "one",
            &format!(
                r#"
[client]
identity = "client.key"

[[servers]]
name = "home"
addr = "127.0.0.1:8443"
server_name = "localhost"
server_id = "{}"
ca = "dev-ca.pem"
"#,
                id_hex()
            ),
        );

        let config = ClientConfig::load(&path).unwrap();
        assert_eq!(config.identity, dir.join("client.key"));
        assert_eq!(config.servers.len(), 1);
        assert_eq!(
            config.servers[0].ca.as_deref(),
            Some(dir.join("dev-ca.pem").as_path())
        );
        assert_eq!(config.server(None).unwrap().name, "home");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_duplicate_server_names() {
        let (dir, path) = write(
            "dupe",
            &format!(
                r#"
[[servers]]
name = "home"
addr = "127.0.0.1:1"
server_name = "localhost"
server_id = "{id}"

[[servers]]
name = "home"
addr = "127.0.0.1:2"
server_name = "localhost"
server_id = "{id}"
"#,
                id = id_hex()
            ),
        );

        assert!(ClientConfig::load(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_invalid_server_id() {
        let (dir, path) = write(
            "bad-id",
            r#"
[[servers]]
name = "home"
addr = "127.0.0.1:1"
server_name = "localhost"
server_id = "not hex"
"#,
        );

        assert!(ClientConfig::load(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn requires_a_server_name_when_ambiguous() {
        let (dir, path) = write(
            "multi",
            &format!(
                r#"
[[servers]]
name = "a"
addr = "127.0.0.1:1"
server_name = "localhost"
server_id = "{id}"

[[servers]]
name = "b"
addr = "127.0.0.1:2"
server_name = "localhost"
server_id = "{id}"
"#,
                id = id_hex()
            ),
        );

        let config = ClientConfig::load(&path).unwrap();
        assert!(config.server(None).is_err());
        assert_eq!(config.server(Some("b")).unwrap().name, "b");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_settings_load_from_toml_and_resolve_paths() {
        let (dir, path) = write(
            "server",
            r#"
[server]
addr = "127.0.0.1:9000"
data_dir = "state"
tls_cert = "tls/server.crt"
tls_key = "tls/server.key"
authorized_clients = "clients.txt"
max_conn = 32
"#,
        );

        let settings = ServerSettings::resolve(Some(&path), ServerOverrides::default()).unwrap();
        assert_eq!(settings.addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(settings.data_dir, dir.join("state"));
        assert_eq!(settings.tls_cert, Some(dir.join("tls/server.crt")));
        assert_eq!(settings.tls_key, Some(dir.join("tls/server.key")));
        assert_eq!(settings.authorized_clients, dir.join("clients.txt"));
        assert_eq!(settings.max_conn, 32);
        assert!(!settings.dev);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cli_overrides_server_config() {
        let (dir, path) = write(
            "server-override",
            r#"
[server]
addr = "127.0.0.1:9000"
max_conn = 32
dev = true
authorized_clients = "clients.txt"
"#,
        );

        let overrides = ServerOverrides {
            addr: Some("127.0.0.1:9001".parse().unwrap()),
            max_conn: Some(1),
            ..Default::default()
        };

        let settings = ServerSettings::resolve(Some(&path), overrides).unwrap();
        assert_eq!(settings.addr, "127.0.0.1:9001".parse().unwrap());
        assert_eq!(settings.max_conn, 1);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_config_rejects_dev_with_tls_material() {
        let (dir, path) = write(
            "server-dev-tls",
            r#"
[server]
dev = true
tls_cert = "cert.pem"
tls_key = "key.pem"
authorized_clients = "clients.txt"
"#,
        );

        assert!(ServerSettings::resolve(Some(&path), ServerOverrides::default()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_config_requires_an_allowlist() {
        let (dir, path) = write(
            "server-no-allow",
            r#"
[server]
dev = true
"#,
        );

        assert!(ServerSettings::resolve(Some(&path), ServerOverrides::default()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_config_rejects_zero_connections() {
        let (dir, path) = write(
            "server-zero",
            r#"
[server]
dev = true
max_conn = 0
authorized_clients = "clients.txt"
"#,
        );

        assert!(ServerSettings::resolve(Some(&path), ServerOverrides::default()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn server_settings_can_be_entirely_cli_driven() {
        let overrides = ServerOverrides {
            dev: true,
            authorized_clients: Some(PathBuf::from("clients.txt")),
            ..Default::default()
        };

        let settings = ServerSettings::resolve(None, overrides).unwrap();
        assert_eq!(settings.addr, "127.0.0.1:8443".parse().unwrap());
        assert_eq!(settings.max_conn, 128);
        assert_eq!(
            settings.data_dir,
            std::env::current_dir().unwrap().join("data")
        );
        assert!(settings.dev);
    }

    #[test]
    fn checked_in_dev_configs_are_consistent() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("config");

        let server =
            ServerSettings::resolve(Some(&base.join("server.toml")), ServerOverrides::default())
                .expect("example server.toml should parse");
        let client = ClientConfig::load(&base.join("client.toml"))
            .expect("example client.toml should parse");

        let server_identity = identity::Identity::load(&server.identity_key)
            .expect("dev server identity should load");
        let client_identity =
            identity::Identity::load(&client.identity).expect("dev client identity should load");

        let profile = client.server(Some("dev")).unwrap();
        assert_eq!(profile.server_id, server_identity.id());
        assert_eq!(server.addr, profile.addr);

        let allowed = identity::load_allowed_clients(&server.authorized_clients)
            .expect("allowlist should load");
        assert!(
            allowed.contains(&client_identity.id()),
            "authorized_clients.txt must contain the dev client id"
        );

        assert_eq!(server.data_dir, base.join("state/server"));
        assert_eq!(
            profile.ca.as_deref(),
            Some(base.join("state/server/tls/dev-ca.cert.pem").as_path())
        );
    }
}
