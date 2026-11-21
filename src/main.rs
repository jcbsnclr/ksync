use anyhow::Result;
use clap::{Parser, Subcommand};
use ksync::server::{self, ServerArgs};

#[derive(Parser, Debug)]
#[command(name = "ksync", version, about = "content-addressed sync")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the TLS listener.
    Server(ServerArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Server(args) => server::run(args).await,
    }
}
