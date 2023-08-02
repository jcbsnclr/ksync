#![feature(io_error_more)]

mod files;
mod server;
mod config;
mod proto;
mod util;
mod sync;

use std::{path::PathBuf, net::SocketAddr};

use chrono::TimeZone;
use clap::Parser;
use tokio::net::TcpStream;

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
        config: PathBuf
    },

    Cli {
        addr: SocketAddr,
        #[command(subcommand)]
        method: Method
    }
}

#[derive(Parser)]
enum Method {
    Get {
        #[arg(short, long)]
        to: PathBuf,
        #[arg(short, long)]
        from: String
    },
    Insert {
        #[arg(short, long)]
        to: String,
        #[arg(short, long)]
        from: PathBuf
    },

    GetListing,
    Clear
}

#[derive(Parser)]
struct Cmdline {
    #[command(subcommand)]
    command: Command
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    // parse command line arguments
    let args = Cmdline::parse();

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
        },

        // user has invoked the command line interface
        Command::Cli { addr, method } => cli(addr, method).await?
    }

    Ok(())
}

async fn cli(addr: SocketAddr, method: Method) -> anyhow::Result<()> {
    // connect to remote server
    let mut stream = TcpStream::connect(addr).await?;

    match method {
        Method::Get { to, from } => {
            let path = files::Path::new(&from)?;
            let response = proto::invoke(&mut stream, server::Get, path).await?;

            tokio::fs::write(to, response).await?;
        },

        Method::Insert { to, from } => {
            let path = files::Path::new(&to)?;
            let data = tokio::fs::read(from).await?;

            proto::invoke(&mut stream, server::Insert, (path, data)).await?;
        },

        Method::GetListing => {
            let list = proto::invoke(&mut stream, server::GetListing, ()).await?;

            for (path, object, timestamp) in list.iter() {
                let timestamp = chrono::Local.timestamp_nanos(*timestamp as i64);

                println!("{}: {} @ {}", path, object.hex(), timestamp);
            }
        },

        Method::Clear => {
            proto::invoke(&mut stream, server::Clear, ()).await?;
        }
    }

    Ok(())
}