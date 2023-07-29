#![feature(io_error_more)]

mod files;
mod server;
mod config;
mod proto;
mod util;

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
    }
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
    //     command: Command::Daemon { config: "example/server.toml".into() }
    // };


    // load and parse the config files

    match args.command {
        Command::Daemon { config } => {
            let config_str = tokio::fs::read_to_string(config).await?;
            let config = toml::from_str(&config_str)?;
        
            // initialise and run the server
            let server = server::Server::init(config).await?;
            server.run().await?;
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
        }
    }

    Ok(())
}