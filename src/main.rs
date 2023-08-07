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
use files::Revision;
use tokio::net::TcpStream;

use server::methods;

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
enum RollbackCommand {
    Time {
        time: u128
    },
    Earliest {
        index: usize
    },
    Latest {
        index: usize
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
    Delete {
        #[arg(short, long)]
        path: String
    },

    GetNode {
        #[arg(short, long)]
        path: String
    },

    GetHistory {
        #[command(subcommand)]
        revision: Option<RollbackCommand>
    },

    GetListing,
    Rollback {
        #[command(subcommand)]
        revision: RollbackCommand
    },
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
            let response = proto::invoke(&mut stream, methods::Get, path).await?;

            tokio::fs::write(to, response).await?;
        },

        Method::Insert { to, from } => {
            let path = files::Path::new(&to)?;
            let data = tokio::fs::read(from).await?;

            proto::invoke(&mut stream, methods::Insert, (path, data)).await?;
        },

        Method::GetHistory { revision } => {
            let history = proto::invoke(&mut stream, methods::GetHistory, ()).await?;

            let range = match revision {
                Some(RollbackCommand::Earliest { index }) => index..history.len(),
                Some(RollbackCommand::Latest { index }) => 0..index,
                Some(RollbackCommand::Time { time }) => {
                    let index = history.iter()
                        .enumerate()
                        .take_while(|(_, &(t,_))| t <= time)
                        .last()
                        .map(|(i,_)| i)
                        .unwrap();

                    0..index
                },

                None => 0..history.len()
            };

            if range.end >= history.len() {

            }

            let slice = &history[range.clone()];

            println!("Filesystem History:");
            for (index, &(timestamp, object)) in range.zip(slice) {
                let timestamp = chrono::Local.timestamp_nanos(timestamp as i64);

                println!("  [{index:04}] revision {} @{} UTC", object.hex(), timestamp.format("%v-%X"));
            }
        }

        Method::GetListing => {
            let list = proto::invoke(&mut stream, methods::GetListing, ()).await?;

            for (path, object, timestamp) in list.iter() {
                let timestamp = chrono::Local.timestamp_nanos(*timestamp as i64);

                if let Some(object) = object {
                    println!("{}: {} @ {}", path, object.hex(), timestamp);
                } else {
                    println!("{}: DELETED @ {}", path, timestamp);
                }
            }
        },

        Method::GetNode { path } => {
            let path = files::Path::new(&path)?;

            let mut node = proto::invoke(&mut stream, methods::GetNode, (path, Revision::FromLatest(0))).await?;

            println!("Entries in {path}:");

            for (path, object, timestamp) in node.file_list()? {
                let timestamp = chrono::Utc.timestamp_nanos(timestamp as i64);

                if let Some(object) = object {
                    println!("  {}: {} @{} UTC", path, object.hex(), timestamp.format("%v_%X"));
                } else {
                    println!("  {}: DELETED @ {}", path, timestamp);
                }
            }
        }

        Method::Clear => {
            proto::invoke(&mut stream, methods::Clear, ()).await?;
        },

        Method::Delete { path } => {
            let path = files::Path::new(&path)?;

            proto::invoke(&mut stream, methods::Delete, path).await?;
        },

        Method::Rollback { revision } => {
            let revision = match revision {
                RollbackCommand::Time { time } => Revision::AsOfTime(time),
                RollbackCommand::Earliest { index } => Revision::FromEarliest(index),
                RollbackCommand::Latest { index } => Revision::FromLatest(index)
            };

            proto::invoke(&mut stream, methods::Rollback, revision).await?;
        }
    }

    Ok(())
}