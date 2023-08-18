use notify::EventKind;
use notify::Watcher;
use tokio::sync::mpsc;

use digest::Digest;

use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use crate::client::Client;
use crate::config;
use crate::files::Node;
use crate::files::Path;
use crate::files::Revision;
use crate::server::methods;

enum SyncEvent {
    Notify(notify::Result<notify::Event>),
    Resync,
}

enum FileStatus {
    Newer,
    Older,
    Same,
    NotPresent,
    Deleted,
}

impl FileStatus {
    fn needs_fetch(&self) -> bool {
        matches!(self, FileStatus::Older)
    }

    fn needs_upload(&self) -> bool {
        matches!(self, FileStatus::Newer)
    }

    fn not_present(&self) -> bool {
        matches!(self, FileStatus::NotPresent)
    }

    fn is_deleted(&self) -> bool {
        matches!(self, FileStatus::Deleted)
    }
}

pub struct SyncClient {
    _watcher: notify::RecommendedWatcher,
    event_queue: mpsc::Receiver<SyncEvent>,
    dir: PathBuf,
    client: Client,
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
        let mut watcher = notify::recommended_watcher(move |e| {
            send.blocking_send(SyncEvent::Notify(e)).unwrap()
        })?;

        // get absolute path of the folder to sync and tell the watcher to watch it
        let dir = config.point.dir.canonicalize()?;
        watcher.watch(&dir, notify::RecursiveMode::Recursive)?;

        // open connection with server
        let mut client = Client::connect(config.remote).await?;

        // spawn thread to request a re-sync. time between re-syncs is configurable
        tokio::spawn(async move {
            let send = send_timer;

            loop {
                // wait configured number of seconds, and then send resync event to sync client
                tokio::time::sleep(tokio::time::Duration::from_secs(config.resync_time)).await;
                send.send(SyncEvent::Resync).await.unwrap();
            }
        });

        // load the client key
        let key = tokio::fs::read(config.key).await?;
        let key = bincode::deserialize(&key)?;

        // authenticate with the server using client key
        client.invoke(methods::auth::Identify, key).await?;

        Ok(SyncClient {
            _watcher: watcher,
            event_queue,
            dir,
            client,
        })
    }

    /// Takes a path relative to the server and produces a local path into the sync point
    fn local_path(&self, path: Path) -> PathBuf {
        self.dir.join(&path.as_str()[1..])
    }

    /// Takes a path within the sync point folder and produces a path relative to the server
    fn remote_path<'a>(&self, path: &std::path::Path) -> String {
        let new_path = path.strip_prefix(&self.dir).unwrap();
        let as_str = new_path.to_str().unwrap();

        format!("/{}", as_str)
    }

    /// Fetch a given file from the server
    async fn fetch_file(&mut self, path: Path<'_>) -> anyhow::Result<()> {
        let data = self.client.invoke(methods::fs::Get, path).await?;

        let (parent, _) = path.parent_child();
        let local_parent = self.local_path(parent);
        let local_path = self.local_path(path);

        tokio::fs::create_dir_all(local_parent).await?;
        tokio::fs::write(local_path, &data).await?;

        Ok(())
    }

    /// Update a given file on the server
    async fn upload_file(&mut self, path: Path<'_>) -> anyhow::Result<()> {
        let local_path = self.local_path(path);

        let data = tokio::fs::read(local_path).await?;

        self.client
            .invoke(methods::fs::Insert, (path, data))
            .await?;

        Ok(())
    }

    /// Delete a given file locally
    async fn delete_file(&mut self, path: Path<'_>) -> anyhow::Result<()> {
        let local_path = self.local_path(path);

        if local_path.is_dir() {
            tokio::fs::remove_dir_all(local_path).await?;
        } else if local_path.is_file() {
            tokio::fs::remove_file(local_path).await?;
        }

        Ok(())
    }

    /// Compare the local and remote copy of a file, returning it's status relative to the server
    async fn compare(&mut self, root: &Node, path: Path<'_>) -> anyhow::Result<FileStatus> {
        let local_path = self.local_path(path);

        // handle the file existing locally, but not on the server
        // TODO: re-structure this to be prettier
        if let Some(file) = root.traverse(path)? {
            if file.data().is_none() || file.dir().is_some() {
                if !local_path.exists() {
                    return Ok(FileStatus::Same);
                }
                return Ok(FileStatus::Deleted);
            }
        } else {
            return Ok(FileStatus::Newer);
        };

        if local_path.exists() {
            // check if the file exists on the server
            if let Some(file) = root.traverse(path)? {
                if file.data().is_none() || file.dir().is_some() {
                    return Ok(FileStatus::Deleted);
                }

                let object = file.file().unwrap();

                // read the contents of the file and calculate SHA-256 hash
                let data = tokio::fs::read(&local_path).await?;

                let mut hasher = sha2::Sha256::new();
                hasher.update(&data);
                let local_hash = hasher.finalize();

                // check if the hashes match
                let hash_match = &local_hash[..] == object.hash();

                if hash_match {
                    // hashes match; nothing to be done
                    Ok(FileStatus::Same)
                } else {
                    // get local and remote timestamps
                    let local_metadata = tokio::fs::metadata(&local_path).await?;
                    let local_time = local_metadata.modified()?;
                    let remote_time =
                        SystemTime::UNIX_EPOCH + Duration::from_nanos(file.timestamp() as u64);

                    if local_time > remote_time {
                        // local copy newer than remote copy
                        Ok(FileStatus::Newer)
                    } else {
                        // local copy older than remote copy
                        // NOTE: if the local and remote timestamps match, then we assume the server's copy is newer, due to latency between client -> server sync
                        Ok(FileStatus::Older)
                    }
                }
            } else {
                Ok(FileStatus::Newer)
            }
        } else {
            Ok(FileStatus::NotPresent)
        }
    }

    /// Bring an individual file in sync with the remote server
    async fn resync_file(&mut self, root: &Node, path: Path<'_>) -> anyhow::Result<()> {
        log::debug!("re-syncing file '{path}");

        let status = self.compare(root, path).await?;

        if status.needs_fetch() {
            log::info!("local copy of '{path}' is out of date; fetching from server");
            self.fetch_file(path).await?;
        } else if status.not_present() {
            log::info!("local copy of '{path}' does not exist; fetching from server");
            self.fetch_file(path).await?;
        } else if status.needs_upload() {
            log::info!("remote copy of '{path}' is out of date; uploading to server");
            self.upload_file(path).await?;
        } else if status.is_deleted() {
            log::info!("remote file '{path}' deleted; deleting local copy");
            self.delete_file(path).await?;
        }

        Ok(())
    }

    /// The intial re-sync, used to catch any files that were added since the synchronisation client was last ran
    async fn init_resync(&mut self) -> anyhow::Result<()> {
        log::info!("perfoming initial re-sync");

        // get an iterator over the files in the sync folder
        let dir_str = self.dir.to_str().unwrap();
        let files = glob::glob(&format!("{}/**/*", dir_str))?;

        // retrieve the remote filesystem structure
        let listing = self
            .client
            .invoke(
                methods::fs::GetNode,
                (Path::new("/")?, Revision::FromLatest(0)),
            )
            .await?;

        for entry in files {
            let entry = entry?;

            if !entry.is_file() {
                // we only want to process files; empty folders will not be synced to the server, and likewise, empty folders on the server will not be created on the client
                continue;
            }

            let path = self.remote_path(entry.as_path());
            let path = Path::new(&path)?;

            // resync the file
            self.resync_file(&listing, path).await?;
        }

        Ok(())
    }

    /// Re-synchronise local sync folder with remote files
    async fn resync(&mut self) -> anyhow::Result<()> {
        log::info!("re-syncing with server");
        // retrieve server's file listing
        let listing = self
            .client
            .invoke(
                methods::fs::GetNode,
                (Path::new("/")?, Revision::FromLatest(0)),
            )
            .await?;

        for (path, node) in listing.iter() {
            let path = Path::new(&path)?;

            if node.file().is_some() || node.data().is_none() {
                // the node is either a file, or has been deleted, so must be processed
                self.resync_file(&listing, path).await?;
            }
        }

        Ok(())
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        self.init_resync().await?;

        // receive messages from event queue
        while let Some(event) = self.event_queue.recv().await {
            match event {
                // a file has been updated in the sync folder
                SyncEvent::Notify(event) => {
                    let event = event?;

                    // we only want to handle modification events rn
                    if !matches!(event.kind, EventKind::Modify(_)) {
                        continue;
                    }

                    log::trace!("got event {:#?}", event);

                    // create map of remote path -> metadata
                    let mut files = self
                        .client
                        .invoke(
                            methods::fs::GetNode,
                            (Path::new("/")?, Revision::FromLatest(0)),
                        )
                        .await?;
                    let files = files.file_list()?.as_map();

                    // iterate over files in event
                    for path in event.paths {
                        // make sure the event is for a file that is in the sync folder
                        if path.starts_with(&self.dir) && path.is_file() {
                            // strip sync folder from file's path to determine it's remote path
                            let remote_path =
                                format!("/{}", path.strip_prefix(&self.dir)?.to_str().unwrap());
                            // let remote_path = Path::new(&remote_path)?;

                            log::info!(
                                "path: {}, remote_path: {}",
                                path.to_string_lossy(),
                                remote_path
                            );

                            // read the contents of the file
                            let data = tokio::fs::read(&path).await?;

                            // calculate hash of file's contents
                            let mut hasher = sha2::Sha256::new();
                            hasher.update(&data);
                            let hash: [u8; 32] = hasher.finalize().try_into().unwrap();

                            // if the server's copy of the file's hash matches the local copy, do nothing
                            if let Some((object, _)) = files.get(&remote_path) {
                                if object.hash() == &hash {
                                    continue;
                                }
                            }

                            // upload file to server
                            log::info!(
                                "inserting file {} -> {remote_path}",
                                path.to_string_lossy()
                            );
                            self.client
                                .invoke(methods::fs::Insert, (Path::new(&remote_path)?, data))
                                .await?;
                        }
                    }
                }

                // periodic resync requested
                SyncEvent::Resync => {
                    self.resync().await?;
                }
            }
        }

        Ok(())
    }
}
