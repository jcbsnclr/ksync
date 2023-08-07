use notify::EventKind;
use notify::Watcher;
use tokio::sync::mpsc;
use tokio::net;

use digest::Digest;

use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use crate::config;
use crate::files::Path;
use crate::server::methods;
use crate::proto;

enum SyncEvent {
    Notify(notify::Result<notify::Event>),
    Resync
}

pub struct SyncClient {
    _watcher: notify::RecommendedWatcher,
    event_queue: mpsc::Receiver<SyncEvent>,
    dir: PathBuf,
    remote: net::TcpStream
}

impl SyncClient {
    /// Initialise the sync client from a given config
    pub async fn init(config: config::Sync) -> anyhow::Result<SyncClient> {
        // create the channel used to send events to the sync client
        let (send, event_queue) = mpsc::channel(1024);
        send.send(SyncEvent::Resync).await?;

        // init_resync only re-syncs files that already exist locally; perform a proper re-sync afterwards to retrieve any files we missed
        let send_timer = send.clone();

        // initialise the watcher; tell it to notify 
        let mut watcher = notify::recommended_watcher(
            move |e| send.blocking_send(SyncEvent::Notify(e)).unwrap()
        )?;

        // get absolute path of the folder to sync and tell the watcher to watch it
        let dir = config.point.dir.canonicalize()?;
        watcher.watch(&dir, notify::RecursiveMode::Recursive)?;

        // open connection with server
        let remote = net::TcpStream::connect(config.remote).await?;

        // spawn thread to request a re-sync. time between re-syncs is configurable
        tokio::spawn(async move {
            let send = send_timer;

            loop {
                // wait configured number of seconds, and then send resync event to sync client
                tokio::time::sleep(tokio::time::Duration::from_secs(config.resync_time)).await;
                send.send(SyncEvent::Resync).await.unwrap();
            }
        });

        Ok(SyncClient { _watcher: watcher, event_queue, dir, remote })
    }

    /// Performs the initial re-sync between the client and server, ensuring both are up-to-date with any local/remote changes
    async fn init_resync(&mut self) -> anyhow::Result<()> {
        log::info!("performing initial resync from server");

        // list all files in the folder to sync
        let files = glob::glob(&format!("{}/**/*", self.dir.to_str().unwrap()))?;

        log::info!("getting file listing from server");

        // retrieve server's file listing
        let listing = proto::invoke(&mut self.remote, methods::GetListing, ()).await?;
        let listing = listing.as_map();

        for path in files.filter_map(Result::ok).filter(|p| p.is_file()) {
            // work out the remote path of a local file
            let remote_path = format!("/{}", path.strip_prefix(&self.dir)?.to_str().unwrap());
            let remote_path = Path::new(&remote_path)?;

            log::info!("processing file {}", remote_path);

            // fetch the file's metadata
            let metadata = tokio::fs::metadata(&path).await?;

            // work out the hash of the file's contents
            let contents = tokio::fs::read(&path).await?;
            let mut hasher = sha2::Sha256::new();
            hasher.update(&contents);
            let hash: [u8; 32] = hasher.finalize().try_into().unwrap();

            if let Some((object, timestamp)) = listing.get(&remote_path) {
                if let Some(object) = object {
                    // file exists on the remote server
                    if object.hash() != &hash && timestamp > &metadata.modified()? {
                        // the local copy of the file is out of date
                        log::info!("local copy of {remote_path} out of date; retrieving from server");

                        // fetch server's copy and store to disk
                        let data = proto::invoke(&mut self.remote, methods::Get, remote_path).await?;
                        tokio::fs::write(path, &data).await?;
                    } else if object.hash() != &hash && timestamp < &metadata.modified()? {
                        // the local copy of the file is newer than the remote copy
                        log::info!("local copy of {remote_path} newer than remote copy; uploading to server");

                        // upload local copy to server
                        proto::invoke(&mut self.remote, methods::Insert, (remote_path, contents)).await?;
                    }
                } else {
                    // file has been deleted
                    tokio::fs::remove_file(path).await?;
                }
            } else {
                // local file does not exist on server
                log::info!("local copy of {remote_path} does not exist in remote; uploading to server");
                
                // upload it to the server
                proto::invoke(&mut self.remote, methods::Insert, (remote_path, contents)).await?;
            }
        }

        Ok(())
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        // perform an initial resync to bring client/server up-to-date
        self.init_resync().await?;

        // receive messages from event queue
        while let Some(event) = self.event_queue.recv().await {
            match event {
                // a file has been updated in the sync folder
                SyncEvent::Notify(event) => {
                    let event = event?;

                    // we only want to handle modification events rn
                    if !matches!(event.kind, EventKind::Modify(_)) {
                        continue
                    }

                    log::info!("got event {:#?}", event);

                    // create map of remote path -> metadata
                    let files = proto::invoke(&mut self.remote, methods::GetListing, ()).await?;
                    let files = files.as_map();
    
                    // iterate over files in event
                    for path in event.paths {
                        // make sure the event is for a file that is in the sync folder
                        if path.starts_with(&self.dir) && path.is_file() {
                            // strip sync folder from file's path to determine it's remote path
                            let remote_path = format!("/{}", path.strip_prefix(&self.dir)?.to_str().unwrap());
                            let remote_path = Path::new(&remote_path)?;
    
                            log::info!("path: {}, remote_path: {}", path.to_string_lossy(), remote_path);
    
                            // read the contents of the file
                            let data = tokio::fs::read(&path).await?;

                            // calculate hash of file's contents
                            let mut hasher = sha2::Sha256::new();
                            hasher.update(&data);
                            let hash: [u8; 32] = hasher.finalize().try_into().unwrap();
            
                            // if the server's copy of the file's hash matches the local copy, do nothing
                            if let Some((Some(object), _)) = files.get(&remote_path)  {
                                if object.hash() == &hash {
                                    continue
                                }
                            }

                            // upload file to server
                            log::info!("inserting file {} -> {remote_path}", path.to_string_lossy());
                            proto::invoke(&mut self.remote, methods::Insert, (remote_path, data)).await?;
                        }
                    }
                },

                // periodic resync requested
                SyncEvent::Resync => {
                    log::info!("re-syncing with server");

                    // get file listing from server and create iterator over it 
                    let files = proto::invoke(&mut self.remote, methods::GetListing, ()).await?;
                    let files = files.iter();

                    for (path, object, timestamp) in files {
                        let remote_path = Path::new(path)?;

                        // strip first char from path, and join it to the sync folder, to get the file's location
                        let local_path = self.dir.join(&path[1..]);

                        log::info!("file: {}", local_path.to_string_lossy());

                        if !local_path.exists() && !object.is_none() {
                            // file does not exist locally; recursively make folders, and fetch from server
                            tokio::fs::create_dir_all(local_path.parent().unwrap_or(&self.dir)).await?;
                            let data = proto::invoke(&mut self.remote, methods::Get, remote_path).await?;
                            tokio::fs::write(local_path, data).await?;
                        } else if local_path.exists() {
                            // file exists locally

                            // fetch metadata
                            let metadata = tokio::fs::metadata(&local_path).await?;

                            // calculate the hash of the file's contents
                            let contents = tokio::fs::read(&local_path).await?;
                            let mut hasher = sha2::Sha256::new();
                            hasher.update(&contents);
                            let hash: [u8; 32] = hasher.finalize().try_into().unwrap();

                            // work out timestamp based on UNIX epoch + offset
                            let time = SystemTime::UNIX_EPOCH + Duration::from_nanos(*timestamp as u64);

                            // check if local and remote hashes match, and compare timestamps
                            if let Some(object) = object {
                                if &hash != object.hash() && time > metadata.modified()? {
                                    // local file is out of date; fetch from server
                                    let data = proto::invoke(&mut self.remote, methods::Get, remote_path).await?;
                                    tokio::fs::write(&local_path, data).await?;
                                }
                            } else {
                                // file has been deleted
                                log::info!("file {remote_path} has been deleted; deleting local copy");
                                tokio::fs::remove_file(&local_path).await?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}