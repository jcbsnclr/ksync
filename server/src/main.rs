mod files;
mod server;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
struct Cmdline {
    #[command(subcommand)]
    command: Command
}

#[derive(clap::Subcommand)]
enum Command {
    Add {
        file: PathBuf,
    },
    ListTree,
    ListLinks,
    GetLink { file: PathBuf }
}

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let server = server::Server::init(server::ServerConfig {
        addr: ([127, 0, 0, 1], 8080).into(),
        db: PathBuf::from("/tmp/testdb")
    }).await?;

    server.run().await?;

    Ok(())
}
