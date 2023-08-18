use chrono::TimeZone;
use clap::Parser;

use ring::rand::SystemRandom;
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};

use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::files::{crypto, Files, Path, Revision};

#[derive(Parser)]
pub enum Command {
    Get {
        path: String,
    },

    Insert {
        path: String,
    },

    Ls {
        path: String,
    },

    GenPair {
        out: PathBuf,
    },

    SignKey {
        #[arg(short, long)]
        with: PathBuf,
        #[arg(short, long)]
        key: PathBuf,
    },

    VerifyKey {
        #[arg(short, long)]
        with: PathBuf,
        #[arg(short, long)]
        key: PathBuf,
    },

    SetAdmin {
        key: PathBuf,
    },

    SetServer {
        key: PathBuf,
    },

    Trust {
        key: PathBuf,
    },

    PubKey {
        #[arg(short, long)]
        key: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
    },

    DbgKey {
        key: PathBuf,
    },

    Clear,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Identity {
    identifier: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct KeyGen {
    identity: Identity,
}

pub fn admin_cli(files: &Files, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Clear => {
            files.clear()?;
            eprintln!("database cleared");
        }

        Command::GenPair { out } => {
            let editor = std::env::var("EDITOR")?;
            let tmp = tempfile::NamedTempFile::new()?;

            let initial_data = toml::to_string(&KeyGen {
                identity: Identity {
                    identifier: "test key".to_string(),
                },
            })?;

            std::fs::write(tmp.path(), &initial_data)?;

            std::process::Command::new(&editor)
                .arg(tmp.path())
                .spawn()?
                .wait()?;

            let data = std::fs::read_to_string(tmp.path())?;

            let data: KeyGen = toml::from_str(&data)?;

            let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();

            let data = bincode::serialize(
                &crypto::Key::from_key_pair(key.as_ref(), &data.identity.identifier).unwrap(),
            )?;

            std::fs::write(out, data)?;
        }

        Command::SignKey { with, key } => {
            let with_data = std::fs::read(with)?;
            let key_data = std::fs::read(&key)?;

            let withk: crypto::Key = bincode::deserialize(&with_data)?;
            let mut keyk: crypto::Key = bincode::deserialize(&key_data)?;

            keyk.sign(&withk)?;

            let data = bincode::serialize(&keyk)?;

            std::fs::write(key, data)?;
        }

        Command::VerifyKey { with, key } => {
            let with_data = std::fs::read(with)?;
            let key_data = std::fs::read(&key)?;

            let withk: crypto::Key = bincode::deserialize(&with_data)?;
            let keyk: crypto::Key = bincode::deserialize(&key_data)?;

            if keyk.verify(&withk)? {
                eprintln!("key verified");
            } else {
                eprintln!("error verifying key");
            }
        }

        Command::SetAdmin { key } => {
            let key_data = std::fs::read(key)?;
            let key: crypto::Key = bincode::deserialize(&key_data)?;

            files.set_admin_key(key)?;
        }

        Command::SetServer { key } => {
            let key_data = std::fs::read(key)?;
            let key: crypto::Key = bincode::deserialize(&key_data)?;

            files.set_server_key(key)?;
        }

        Command::DbgKey { key } => {
            let key_data = std::fs::read(key)?;

            let keyk: crypto::Key = bincode::deserialize(&key_data)?;

            println!("{keyk}");
        }

        Command::PubKey { key, out } => {
            let key_data = std::fs::read(key)?;
            let key: crypto::Key = bincode::deserialize(&key_data)?;

            let pub_key = key.pub_key();
            let pub_key = bincode::serialize(&pub_key)?;

            std::fs::write(out, pub_key)?;
        }

        Command::Trust { key } => {
            let key_data = std::fs::read(key)?;
            let key: crypto::Key = bincode::deserialize(&key_data)?;

            let pub_key = key.pub_key();

            files.trust_client(pub_key)?;
        }

        Command::Get { path } => {
            let path = Path::new(&path)?;

            let data = files.get(path, Revision::FromLatest(0))?;

            if let Some(data) = data {
                io::stdout().write_all(&data[..])?;
            } else {
                eprintln!("error: file '{path}' not found");
            }
        }

        Command::Insert { path } => {
            let path = Path::new(&path)?;

            let mut data = vec![];
            io::stdin().read_to_end(&mut data)?;

            files.insert(path, &data)?;

            eprintln!("wrote to file {path}");
        }

        Command::Ls { path } => {
            let path = Path::new(&path)?;

            let node = files.get_node(path, Revision::FromLatest(0))?;

            match node {
                Some(mut node) => {
                    if node.dir_mut().is_some() {
                        println!("Entries under {path}:");

                        for (path, object, timestamp) in node.file_list()? {
                            let timestamp = chrono::Local.timestamp_nanos(timestamp as i64);

                            println!("  {path}: {} @ {}", object.hex(), timestamp.format("%v-%X"));
                        }
                    } else {
                        eprintln!("Error: '{path}' is not a directory");
                    }
                }
                None => eprintln!("Error: '{path}' not found"),
            }
        }
    }

    Ok(())
}
