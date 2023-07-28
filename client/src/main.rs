#![feature(io_error_more)]

mod config;

use std::collections::HashMap;
use std::path::PathBuf;

use notify::Watcher;
use tokio::net;
use tokio::sync;

use common::proto;

use clap::Parser;

#[derive(Parser)]
struct Cmdline {
    #[arg(short, long)]
    config: PathBuf
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // parse command line args and config file
    let args = Cmdline::parse();
    let config_str = tokio::fs::read_to_string(args.config).await?;
    let config: config::Config = toml::from_str(&config_str)?;

    // connect to server
    let mut stream = net::TcpStream::connect(config.client.remote).await?;

    // open channel for file notifications
    let (send, mut recv) = sync::mpsc::channel(1024);

    // set up inotify watcher
    let mut watcher = notify::recommended_watcher(move |e| send.blocking_send(e).unwrap() )?;
    let mut remote_links = HashMap::new();

    for config::Sync { to, from } in config.sync.iter() {
        // insert file paths and their remote link into hashmap
        remote_links.insert(from.canonicalize()?, to);
        watcher.watch(from, notify::RecursiveMode::NonRecursive)?;
    }

    loop {
        match recv.recv().await {
            Some(event) => {
                let event = event?;
                dbg!(&event);
                
                for path in event.paths {
                    let remote_link = remote_links.get(&path).unwrap();
                    let data = tokio::fs::read(&path).await?;

                    proto::write_packet(&mut stream, "INSERT", (remote_link, data)).await?;

                    let response = proto::read_packet(&mut stream).await?.expect("got no response");

                    assert_eq!(response.method, "OK");
                }
            }

            // closed; break
            None => break
        }
    }

    Ok(())
}