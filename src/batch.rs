use chrono::TimeZone;
use clap::Parser;

use std::path::PathBuf;

use crate::client::Client;
use crate::server::methods;
use crate::files::{self, Revision, crypto};

#[derive(Parser)]
pub enum RollbackCommand {
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
pub enum Method {
    Identify {
        #[arg(short, long)]
        key: PathBuf
    },

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

    Rollback {
        #[command(subcommand)]
        revision: RollbackCommand
    },
    Clear
}

pub async fn run_method(client: &mut Client, method: Method) -> anyhow::Result<()> {
    match method {
        Method::Identify { key } => {
            let key = tokio::fs::read(&key).await?;
            let key: crypto::Key = bincode::deserialize(&key)?;

            client.invoke(methods::auth::Identify, key).await?;
        }

        Method::Get { to, from } => {
            let path = files::Path::new(&from)?;
            let response = client.invoke(methods::fs::Get, path).await?;

            tokio::fs::write(to, response).await?;
        },

        Method::Insert { to, from } => {
            let path = files::Path::new(&to)?;
            let data = tokio::fs::read(from).await?;

            client.invoke(methods::fs::Insert, (path, data)).await?;
        },

        Method::GetHistory { revision } => {
            let history = client.invoke(methods::fs::GetHistory, ()).await?;

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
        },

        Method::GetNode { path } => {
            let path = files::Path::new(&path)?;

            let mut node = client.invoke(methods::fs::GetNode, (path, Revision::FromLatest(0))).await?;

            println!("Entries in {path}:");

            for (path, object, timestamp) in node.file_list()? {
                let timestamp = chrono::Utc.timestamp_nanos(timestamp as i64);

                println!("  {}: {} @{} UTC", path, object.hex(), timestamp.format("%v_%X"));
            }
        }

        Method::Clear => {
            client.invoke(methods::fs::Clear, ()).await?;
        },

        Method::Delete { path } => {
            let path = files::Path::new(&path)?;

            client.invoke(methods::fs::Delete, path).await?;
        },

        Method::Rollback { revision } => {
            let revision = match revision {
                RollbackCommand::Time { time } => Revision::AsOfTime(time),
                RollbackCommand::Earliest { index } => Revision::FromEarliest(index),
                RollbackCommand::Latest { index } => Revision::FromLatest(index)
            };

            client.invoke(methods::fs::Rollback, revision).await?;
        }
    }

    Ok(())
}

pub async fn run_batch(client: &mut Client, batch: &str) -> anyhow::Result<()> {
    for line in batch.lines() {
        let args = std::iter::once("").chain(line.split_whitespace());

        let method = Method::parse_from(args);

        run_method(client, method).await?;
    }

    Ok(())
}