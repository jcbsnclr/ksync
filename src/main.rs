#![feature(io_error_more)]

mod files;
mod server;
mod config;
mod proto;
mod util;
mod sync;

use std::{path::PathBuf, net::SocketAddr};

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

    GetTree
}

#[derive(Parser)]
struct Cmdline {
    #[command(subcommand)]
    command: Command
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Cmdline::parse();

    // test configuration, hard-coded 
    // env_logger::Builder::new()
    //     .filter_level(log::LevelFilter::Debug)
    //     .init();
    // let args = Cmdline {
    //     command: Command::Cli { addr: "127.0.0.1:8080".parse().unwrap(), method: Method::Get { to: "outp.txt".into(), from: "/files/test.txt".to_owned() } }
    // };

    // env_logger::Builder::new()
    //     .filter_level(log::LevelFilter::Debug)
    //     .init();
    // let args = Cmdline {
    //     command: Command::Daemon { config: "example/client.toml".into() }
    // };


    // load and parse the config files

    match args.command {
        Command::Daemon { config } => {
            let config_str = tokio::fs::read_to_string(config).await?;
            let config: config::Config = toml::from_str(&config_str)?;

            if config.server.is_none() && config.sync.is_none() {
                eprintln!("error: no server or sync configuration; nothing to do");
            }

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

        Command::Cli { addr, method } => cli(addr, method).await?
    }

    Ok(())
}

async fn cli(addr: SocketAddr, method: Method) -> anyhow::Result<()> {
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

        Method::GetTree => {
            let list = proto::invoke(&mut stream, server::GetTree, ()).await?;

            for (path, object) in list {
                println!("{}: {}", path, object.hex());
            }
        }
    }

    Ok(())
}