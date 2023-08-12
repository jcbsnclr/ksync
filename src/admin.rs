use chrono::TimeZone;
use clap::Parser;

use std::io::{self, Read, Write};

use crate::files::{Files, Path, Revision};

#[derive(Parser)]
pub enum Command {
    Get {
        path: String,
    },

    Insert {
        path: String
    },

    Ls {
        path: String
    },

    Clear
}

pub fn admin_cli(files: &Files, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Clear => {
            files.clear()?;
            eprintln!("database cleared");
        },

        Command::Get { path } => {
            let path = Path::new(&path)?;

            let data = files.get(path, Revision::FromLatest(0))?;

            if let Some(data) = data {
                io::stdout().write_all(&data[..])?;
            } else {
                eprintln!("error: file '{path}' not found");
            }
        },

        Command::Insert { path } => {
            let path = Path::new(&path)?;

            let mut data = vec![];
            io::stdin().read_to_end(&mut data)?;

            files.insert(path, &data)?;

            eprintln!("wrote to file {path}");
        },

        Command::Ls { path } => {
            let path = Path::new(&path)?;

            let node = files.get_node(path, Revision::FromLatest(0))?;

            match node {
                Some(mut node) => {
                    if node.dir().is_some() {
                        println!("Entries under {path}:");

                        for (path, object, timestamp) in node.file_list()? {
                            let timestamp = chrono::Local.timestamp_nanos(timestamp as i64);

                            if let Some(object) = object {
                                println!("  {path}: {} @ {}", object.hex(), timestamp.format("%v-%X"));
                            } else {
                                println!("  {path}: DELETED @ {}", timestamp.format("%v-%X"));
                            }
                        }
                    } else {
                        eprintln!("Error: '{path}' is not a directory");
                    }
                },
                None => eprintln!("Error: '{path}' not found")
            }
        }
    }

    Ok(())
}