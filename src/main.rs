#![feature(io_error_more, async_fn_in_trait)]

mod admin;
mod cli;
mod client;
mod config;
mod files;
mod proto;
mod server;
mod sync;
mod util;

use std::io;
use std::{net::SocketAddr, path::PathBuf};

use clap::Parser;

// fn print_files(files: &files::Files) -> sled::Result<()> {
//     println!("objects:");
//     for entry in files.objects() {
//         let (object, _) = entry?;

//         println!("  * {}", object.hex());
//     }

//     println!("links:");
//     for entry in files.links() {
//         let (name, object) = entry?;

//         println!("  * '{}' -> {}", name, object.hex());
//     }

//     Ok(())
// }

#[derive(Parser)]
enum Command {
    Daemon,

    Cli {
        #[arg(short, long)]
        key: Option<PathBuf>,

        #[arg(short, long)]
        remote: Option<SocketAddr>,

        #[command(subcommand)]
        method: cli::Method,
    },
}

#[derive(Parser)]
struct Cmdline {
    #[arg(short, long)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    // parse command line arguments
    let args = Cmdline::parse();

    let config_path = if let Some(config) = args.config {
        config
    } else {
        log::error!("no config file provided");
        return Ok(());
    };

    let config_str = tokio::fs::read_to_string(&config_path).await?;
    let config: config::Config = toml::from_str(&config_str)?;

    match args.command {
        Command::Daemon => {
            // ksync launched in daemon mode

            let server = tokio::spawn(async {
                // launch server if server config provided
                if let Some(server_config) = config.server {
                    let server = server::Server::init(server_config).await?;
                    server.run().await?;
                }

                Ok::<_, anyhow::Error>(())
            });

            let sync = tokio::spawn(async {
                // launch sync client if sync config provided
                if let Some(sync_config) = config.sync {
                    let mut sync = sync::SyncClient::init(sync_config).await?;
                    sync.run().await?;
                }

                Ok::<_, anyhow::Error>(())
            });

            server.await??;
            sync.await??;
        }

        Command::Cli {
            key,
            remote,
            method,
        } => {
            let key = if let Some(key) = key {
                Some(key)
            } else if let Some(config) = config.client.clone() {
                Some(config.key)
            } else {
                None
            };

            let remote = if let Some(remote) = remote {
                Some(remote)
            } else if let Some(config) = config.client.clone() {
                Some(config.remote)
            } else {
                None
            };

            cli::invoke(key, remote, method).await?;
        }
    }

    Ok(())
}
