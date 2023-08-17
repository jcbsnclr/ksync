#![feature(io_error_more, async_fn_in_trait)]

mod admin;
mod batch;
mod client;
mod config;
mod files;
mod proto;
mod server;
mod sync;
mod util;

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
    Daemon {
        #[arg(short, long)]
        config: PathBuf,
    },

    RunBatch {
        addr: SocketAddr,
        script: PathBuf,
    },

    Cli {
        addr: SocketAddr,
        #[command(subcommand)]
        method: batch::Method,
    },

    Admin {
        db: PathBuf,
        #[command(subcommand)]
        command: admin::Command,
    },
}

#[derive(Parser)]
struct Cmdline {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    // parse command line arguments
    let args = Cmdline::parse();

    // env_logger::builder()
    //     .filter_level(log::LevelFilter::Debug)
    //     .init();
    // let args = Cmdline {
    //     command: Command::Daemon { config: "example/server.toml".into() }
    // };

    match args.command {
        Command::Daemon { config } => {
            // ksync running in daemon mode

            // parse config file
            let config_str = tokio::fs::read_to_string(config).await?;
            let config: config::Config = toml::from_str(&config_str)?;

            if config.server.is_none() && config.sync.is_none() {
                eprintln!("error: no server or sync configuration; nothing to do");
            }

            // spawn file server and sync client if respective configurations supplied

            let server_handle = tokio::spawn(async {
                // if server config provided, initialise and run the server
                if let Some(server_config) = config.server {
                    let server = server::Server::init(server_config).await?;
                    server.run().await?;
                }

                Ok::<_, anyhow::Error>(())
            });

            let sync_handle = tokio::spawn(async {
                // if sync config provided, initialise and run the sync client
                if let Some(sync_config) = config.sync {
                    let mut sync = sync::SyncClient::init(sync_config).await?;
                    sync.run().await?;
                }

                Ok::<_, anyhow::Error>(())
            });

            // wait on tasks to finish before shutting down
            server_handle.await??;
            sync_handle.await??;
        }

        // user has invoked the command line interface
        Command::RunBatch { addr, script } => batch(addr, script).await?,

        Command::Cli { addr, method } => {
            let mut client = client::Client::connect(addr).await?;
            batch::run_method(&mut client, method).await?;
        }

        Command::Admin { db, command } => {
            let files = files::Files::open(db)?;
            admin::admin_cli(&files, command)?;
        }
    }

    Ok(())
}

async fn batch(addr: SocketAddr, script: PathBuf) -> anyhow::Result<()> {
    // connect to remote server
    let mut client = client::Client::connect(addr).await?;
    let script = tokio::fs::read_to_string(&script).await?;

    batch::run_batch(&mut client, &script).await?;

    Ok(())
}
