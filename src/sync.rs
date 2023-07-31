use notify::Watcher;
use tokio::sync::mpsc;
use tokio::net;

use digest::Digest;

use std::path::PathBuf;

use crate::config;
use crate::files::Path;
use crate::server;
use crate::proto;

enum SyncEvent {
    Notify(notify::Result<notify::Event>),
    Retrieve
}

pub struct SyncClient {
    _watcher: notify::RecommendedWatcher,
    event_queue: mpsc::Receiver<SyncEvent>,
    dir: PathBuf,
    remote: net::TcpStream
}

impl SyncClient {
    pub async fn init(config: config::Sync) -> anyhow::Result<SyncClient> {
        let (send, event_queue) = mpsc::channel(1024);
        let send_retrieve = send.clone();
        let mut watcher = notify::recommended_watcher(
            move |e| send.blocking_send(SyncEvent::Notify(e)).unwrap()
        )?;

        let dir = config.point.dir.canonicalize()?;

        watcher.watch(&dir, notify::RecursiveMode::Recursive)?;

        let remote = net::TcpStream::connect(config.remote).await?;

        tokio::spawn(async move {
            let send = send_retrieve;

            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                send.send(SyncEvent::Retrieve).await.unwrap();
            }
        });

        Ok(SyncClient { _watcher: watcher, event_queue, dir, remote })
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        while let Some(event) = self.event_queue.recv().await {
            match event {
                SyncEvent::Notify(event) => {
                    let event = event?;
                    log::info!("got event {:#?}", event);
    
                    for path in event.paths {
                        if path.starts_with(&self.dir) && path.is_file() {
                            let remote_path = path.strip_prefix(&self.dir)?.to_str().unwrap();
                            let remote_path = format!("/{}", remote_path);
                            let remote_path = Path::new(&remote_path)?;
    
                            println!("path: {}, remote_path: {}", path.to_string_lossy(), remote_path);
    
                            let data = tokio::fs::read(&path).await?;
            
                            log::info!("inserting file {} -> {remote_path}", path.to_string_lossy());
                            proto::invoke(&mut self.remote, server::Insert, (remote_path, data)).await?;
                        }
                    }
                },

                SyncEvent::Retrieve => {
                    log::info!("retrieve");

                    println!("bar");
                    let files = proto::invoke(&mut self.remote, server::GetTree, ()).await?;
                    println!("bar");

                    for (path, object) in files.into_iter().map(|(p,o)| ((&p[1..]).to_string(), o)) {
                        println!("foo");
                        let file = self.dir.join(&path);
                        println!("bar");

                        println!("file: {}", file.to_string_lossy());
                        
                        let path = format!("/{path}");

                        if !file.exists() {
                            tokio::fs::create_dir_all(file.parent().unwrap_or(&self.dir)).await?;
                            let data = proto::invoke(&mut self.remote, server::Get, Path::new(&path)?).await?;
                            tokio::fs::write(file, data).await?;
                        } else if file.exists() {
                            let contents = tokio::fs::read(&file).await?;
                            let mut hasher = sha2::Sha256::new();
                            hasher.update(&contents);
                            let hash: [u8; 32] = hasher.finalize().try_into().unwrap();

                            if &hash != object.hash() {
                                let data = proto::invoke(&mut self.remote, server::Get, Path::new(&path)?).await?;
                                tokio::fs::write(&file, data).await?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}