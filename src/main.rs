use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ksync::{
    client,
    config::ClientConfig,
    control::{self, ControlCommand, ControlResult},
    daemon,
    methods::Ping,
    proto::RequestId,
    server::{self, ServerArgs},
};

#[derive(Parser, Debug)]
#[command(name = "ksync", version, about = "content-addressed sync")]
struct Cli {
    /// Client config file. Defaults to $XDG_CONFIG_HOME/ksync/config.toml.
    #[arg(long, global = true, env = "KSYNC_CLIENT_CONFIG")]
    client_config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the TLS server.
    Server(ServerArgs),

    /// Run the per-user sync daemon in the foreground.
    Daemon,

    /// Show daemon and server connection status.
    Status,

    /// Ping a server through the daemon.
    Ping(PingArgs),

    /// Ask the daemon to sync now.
    Sync(SyncArgs),
}

#[derive(clap::Args, Debug)]
struct PingArgs {
    /// Server profile name (optional when only one is configured).
    server: Option<String>,

    /// Bypass the daemon and connect directly (diagnostic).
    #[arg(long)]
    direct: bool,
}

#[derive(clap::Args, Debug)]
struct SyncArgs {
    /// Server profile name (optional when only one is configured).
    server: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Server(args) => server::run(args).await,
        Command::Daemon => daemon::run(config_path(cli.client_config)?).await,
        Command::Status => status(config_path(cli.client_config)?).await,
        Command::Ping(args) => ping(config_path(cli.client_config)?, args).await,
        Command::Sync(args) => sync(config_path(cli.client_config)?, args).await,
    }
}

fn config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    match explicit {
        Some(path) => Ok(path),
        None => {
            let path = ClientConfig::discover()?;
            if !path.exists() {
                bail!(
                    "no config at {} (use --config or KSYNC_CONFIG)",
                    path.display()
                );
            }
            Ok(path)
        }
    }
}

async fn status(path: PathBuf) -> Result<()> {
    let config = ClientConfig::load(&path)?;

    match control::request(&config, ControlCommand::Status).await? {
        ControlResult::Status(status) => {
            println!("daemon pid {}", status.pid);
            for server in status.servers {
                match (server.connected, server.last_error) {
                    (true, _) => println!("  {}: connected", server.name),
                    (false, Some(err)) => println!("  {}: disconnected ({err})", server.name),
                    (false, None) => println!("  {}: disconnected", server.name),
                }
            }
            Ok(())
        }
        ControlResult::Error { code, detail } => bail!("daemon error {code}: {detail}"),
        other => bail!("unexpected status response: {other:?}"),
    }
}

async fn ping(path: PathBuf, args: PingArgs) -> Result<()> {
    let config = ClientConfig::load(&path)?;
    let profile = config.server(args.server.as_deref())?;

    if args.direct {
        let identity = config.identity()?;
        println!("identity {:?}", identity.id());

        let mut tls = client::connect(profile, &identity).await?;
        let started = std::time::Instant::now();
        client::call_core::<Ping>(&mut tls, RequestId(1), ())
            .await
            .context("ping failed")?;
        println!(
            "pong from {} ({:.2?}) [direct]",
            profile.name,
            started.elapsed()
        );
        return Ok(());
    }

    match control::request(
        &config,
        ControlCommand::Ping {
            server: args.server,
        },
    )
    .await?
    {
        ControlResult::Pong { server, rtt_micros } => {
            println!("pong from {server} ({rtt_micros}µs)");
            Ok(())
        }
        ControlResult::Error { code, detail } => bail!("daemon error {code}: {detail}"),
        other => bail!("unexpected ping response: {other:?}"),
    }
}

async fn sync(path: PathBuf, args: SyncArgs) -> Result<()> {
    let config = ClientConfig::load(&path)?;

    match control::request(
        &config,
        ControlCommand::Sync {
            server: args.server,
        },
    )
    .await?
    {
        ControlResult::Ok => {
            println!("sync requested");
            Ok(())
        }
        ControlResult::Error { code, detail } => bail!("daemon error {code}: {detail}"),
        other => bail!("unexpected sync response: {other:?}"),
    }
}
