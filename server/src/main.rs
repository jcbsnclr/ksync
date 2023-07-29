#![feature(io_error_more)]

mod files;
mod server;
mod config;

use std::path::PathBuf;

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
struct Cmdline {
    #[arg(short, long)]
    config: PathBuf
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // env_logger::init();
    // let args = Cmdline::parse();

    // test configuration, hard-coded 
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();
    let args = Cmdline {
        config: "example/server.toml".into()
    };

    // load and parse the config files
    let config_str = tokio::fs::read_to_string(args.config).await?;
    let config = toml::from_str(&config_str)?;

    // initialise and run the server
    let server = server::Server::init(config).await?;
    server.run().await?;

    Ok(())
}
