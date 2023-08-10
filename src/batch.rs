use chrono::TimeZone;
use clap::Parser;

use std::path::PathBuf;

use crate::client::Client;
use crate::server::methods;
use crate::files::{self, Revision};

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

pub async fn run_method(client: &mut Client, method: Method) -> anyhow::Result<()> {
    match method {
        Method::Get { to, from } => {
            let path = files::Path::new(&from)?;
            let response = client.invoke(methods::Get, path).await?;

            tokio::fs::write(to, response).await?;
        },

        Method::Insert { to, from } => {
            let path = files::Path::new(&to)?;
            let data = tokio::fs::read(from).await?;

            client.invoke(methods::Insert, (path, data)).await?;
        },

        Method::GetHistory { revision } => {
            let history = client.invoke(methods::GetHistory, ()).await?;

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
            let list = client.invoke(methods::GetListing, ()).await?;

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

            let mut node = client.invoke(methods::GetNode, (path, Revision::FromLatest(0))).await?;

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
            client.invoke(methods::Clear, ()).await?;
        },

        Method::Delete { path } => {
            let path = files::Path::new(&path)?;

            client.invoke(methods::Delete, path).await?;
        },

        Method::Rollback { revision } => {
            let revision = match revision {
                RollbackCommand::Time { time } => Revision::AsOfTime(time),
                RollbackCommand::Earliest { index } => Revision::FromEarliest(index),
                RollbackCommand::Latest { index } => Revision::FromLatest(index)
            };

            client.invoke(methods::Rollback, revision).await?;
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