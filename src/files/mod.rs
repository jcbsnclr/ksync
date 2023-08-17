pub mod crypto;
pub mod node;

pub use node::*;

use sled::IVec;

use std::fmt::Debug;
use std::io;
use std::path::Path as SysPath;
use std::time::SystemTime;

use chrono::TimeZone;
use digest::Digest;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::util::fmt;

/// An [Object] represents a content-addressable chunk of data in the database, via a SHA-256 hash
#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Object([u8; 32]);

impl Object {
    /// Create an [Object] from a given `hash`
    pub fn from_hash(hash: [u8; 32]) -> Object {
        Object(hash)
    }

    /// Retrieve the hash value of an [Object]
    pub fn hash(&self) -> &[u8; 32] {
        &self.0
    }

    /// Display the [Object] as a hex string
    pub fn hex(&self) -> fmt::HexSlice {
        fmt::HexSlice::from(&self.0[..])
    }
}

impl Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hex())
    }
}

impl<'a> TryFrom<&'a str> for Path<'a> {
    type Error = InvalidPath;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Path::new(value)
    }
}

/// A [Path] in the server's virtual filesystem.
///
/// [Path]s must be a valid UTF-8 string, must be absolute (begin with a `/`), and cannot contain any double slashes (e.g. `/foo//bar`), or a trailing slash
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Hash, PartialEq, Eq)]
#[serde(try_from = "&str")]
pub struct Path<'a>(&'a str);

#[derive(thiserror::Error, Debug)]
#[error("path must begin with a /, and must not contain double slashes")]
pub struct InvalidPath;

impl<'a> Path<'a> {
    /// Create a new [Path] from a given `str`
    ///
    /// # Example
    /// ```rust
    /// use common::Path;
    ///
    /// let root = Path::new("/");
    /// let foo = Path::new("/foo");
    /// let bar = Path::new("bar");
    /// let baz = Path::new("/foo//baz");
    /// let idk = Path::new("/foo/");
    ///
    /// assert!(root.is_ok());
    /// assert!(foo.is_ok());
    /// assert!(bar.is_err());
    /// assert!(baz.is_err());
    /// assert!(idk.is_err());
    /// ```
    pub fn new(str: &'a str) -> Result<Path<'a>, InvalidPath> {
        // count the number of double slashes in the path
        let double_slashes = str.split('/').skip(1).filter(|&p| p == "").count();

        // check validity of path
        if !str.starts_with('/') || (double_slashes != 0 && str != "/") {
            Err(InvalidPath)
        } else {
            Ok(Path(str))
        }
    }

    /// Get a [Path]'s underlying string value
    pub fn as_str(&self) -> &'a str {
        self.0
    }

    /// Returns an [Iterator] over a path's individual components.
    ///
    /// ```rust
    /// use common::Path;
    ///
    /// let test = Path::new("/foo/bar/baz").unwrap();
    /// let mut parts = test.parts();
    ///
    /// assert_eq!(parts.next(), Some("foo"));
    /// assert_eq!(parts.next(), Some("bar"));
    /// assert_eq!(parts.next(), Some("baz"));
    /// assert!(parts.next().is_none());
    /// ```
    pub fn parts(&self) -> impl DoubleEndedIterator<Item = &'a str> {
        (&self.0[1..]).split('/')
    }

    /// Splits a [Path] into a parent and child pair
    ///
    /// ```rust
    /// use common::Path;
    ///
    /// let path = Path::new("/files/test.txt").unwrap();
    /// let (parent, child) = path.parent_child();
    ///
    /// assert_eq!(parent.as_str(), "/files");
    /// assert_eq!(child, Some("test.txt"));
    /// ```
    pub fn parent_child(&'a self) -> (Path<'a>, Option<&'a str>) {
        if self.0 == "/" {
            // if we're the root dir, then return self as parent and no child
            (*self, None)
        } else {
            // find the last slash in the path
            let last_slash = self.0.rfind('/').unwrap();

            let path = if last_slash == 0 {
                // path in the root directory; use root as parent
                Path::new("/").unwrap()
            } else {
                // use path before the last slash as parent
                Path::new(&self.0[0..last_slash]).unwrap()
            };

            (
                path,
                Some(&self.0[last_slash + 1..]), // return path after last slash as the child
            )
        }
    }

    /// Returns an [Iterator] over a [Path]'s ancestors. See [Ancestors] for details
    pub fn ancestors(&'a self) -> Ancestors<'a> {
        Ancestors {
            path: *self,
            iter: 0,
        }
    }
}

/// An [Iterator] over a [Path]'s ancestors, produced with the [Path::ancestors] method.
///
/// ```rust
/// use common::Path;
///
/// let path = Path::new("/foo/bar/baz").unwrap();
/// let mut ancestors = path.ancestors()
///     .map(|path| path.as_str());
///
/// assert_eq!(ancestors.next(), Some("/"));
/// assert_eq!(ancestors.next(), Some("/foo"));
/// assert_eq!(ancestors.next(), Some("/foo/bar"));
/// assert_eq!(ancestors.next(), None)
/// ```
pub struct Ancestors<'a> {
    path: Path<'a>,
    iter: usize,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = Path<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = if self.iter != 0 {
            let index = self
                .path
                .0
                .char_indices()
                .filter(|&(_, c)| c == '/')
                .map(|(i, _)| i)
                .skip(self.iter)
                .next();

            index.map(|index| Path::new(&self.path.0[0..index]).unwrap())
        } else {
            Some(Path::new("/").unwrap())
        };

        self.iter += 1;

        result
    }
}

impl<'a> std::fmt::Display for Path<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn root_merge(_key: &[u8], old_value: Option<&[u8]>, merged_bytes: &[u8]) -> Option<Vec<u8>> {
    let mut list: RootHistory = if let Some(bytes) = old_value {
        bincode::deserialize(bytes).unwrap()
    } else {
        vec![]
    };

    let object = Object::from_hash(merged_bytes.try_into().unwrap());
    let timestamp = SystemTime::UNIX_EPOCH.elapsed().unwrap().as_nanos();

    list.push((timestamp, object));

    Some(bincode::serialize(&list).unwrap())
}

pub struct Files {
    // _db: sled::Db,
    /// A tree that maps an [Object] to it's data
    objects: sled::Tree,
    /// A tree that maps a string "root" name, to an [Object] containing a filesystem [Node]
    roots: sled::Tree,
}

pub type RootHistory = Vec<(u128, Object)>;

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
pub enum Revision {
    FromLatest(usize),
    FromEarliest(usize),
    AsOfTime(u128),
}

// filesystem internals
impl Files {
    /// Opens a [Files] database from a given path, and initialises it
    // TODO: stop re-initialising the database on each open
    pub fn open(path: impl AsRef<SysPath>) -> anyhow::Result<Files> {
        log::info!("opening db at {:?}", path.as_ref());
        let db = sled::open(path)?;
        log::info!("opening objects and roots trees");
        let objects = db.open_tree("objects")?;
        let roots = db.open_tree("roots")?;

        let files = Files { objects, roots };

        files.roots.set_merge_operator(root_merge);

        // if root node does not exist, create it
        if files.roots.get("fs")?.is_none() {
            let dir = Node::new_dir();
            let object = files.serialize(&dir)?;
            files.roots.merge("fs", object.hash())?;
        }

        if files.roots.get("keyring")?.is_none() {
            let dir = Node::new_dir();
            let object = files.serialize(&dir)?;
            files.roots.merge("keyring", object.hash())?;

            files.with_root_mut("keyring", |node| {
                node.make_dir(Path::new("/self")?)?;
                node.make_dir(Path::new("/trusted")?)?;

                Ok(())
            })?;
        }

        Ok(files)
    }

    fn get_root_history(&self, root: &str) -> anyhow::Result<RootHistory> {
        log::info!("loading root '{root}' history");
        let history = self
            .roots
            .get(root)?
            .ok_or(io::Error::new(io::ErrorKind::NotFound, "root not found"))?;

        // deserialise root into it's history
        let history: RootHistory = bincode::deserialize(&history[..])?;

        Ok(history)
    }

    fn get_root(&self, root: &str, revision: Revision) -> anyhow::Result<Node> {
        // load root node from database
        let history = self.get_root_history(root)?;

        // get the right node based on the query
        // TODO: proper handling of no history errors
        let (timestamp, object) = match revision {
            Revision::FromLatest(n) => history.iter().nth_back(n).unwrap(),
            Revision::FromEarliest(n) => history.iter().nth(n).unwrap(),
            Revision::AsOfTime(n) => history.iter().take_while(|(t, _)| t < &n).last().unwrap(),
        };

        // get date-time
        let timestamp = chrono::Local.timestamp_nanos(*timestamp as i64);

        log::info!(
            "found node {} for root '{root}', created {timestamp}",
            object.hex()
        );

        // deserialise node
        let node = self.deserialize(object)?;

        Ok(node)
    }

    fn set_root(&self, root: &str, node: Node) -> anyhow::Result<()> {
        let object = self.serialize(&node)?;
        self.roots.merge(root, object.hash())?;

        log::info!("appended node {} to history of root '{root}'", object.hex());

        Ok(())
    }

    /// Perform operations on a given `root`
    fn with_root_mut<T>(
        &self,
        root: &str,
        op: impl Fn(&mut Node) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        log::info!("mutating root '{root}'");

        // we can only mutate the latest revision of the filesystem.
        let mut node = self.get_root(root, Revision::FromLatest(0))?;

        // perform operation on node
        let result = op(&mut node)?;

        // re-serialize and store new root
        self.set_root(root, node)?;

        Ok(result)
    }

    fn with_root<T>(
        &self,
        root: &str,
        revision: Revision,
        op: impl Fn(&mut Node) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        log::info!("accessing root '{root}'");

        let mut node = self.get_root(root, revision)?;

        // perform operation on node
        let result = op(&mut node)?;

        Ok(result)
    }

    /// Create a new [Object] containing `data`, referenced by it's hash
    fn create_object(&self, data: impl AsRef<[u8]>) -> sled::Result<Object> {
        // generate a hash of data
        let mut hasher = sha2::Sha256::new();
        hasher.update(data.as_ref());
        let hash = hasher.finalize();

        // if there is no object with a given hash, then store data in objects store
        if self.objects.get(hash)?.is_none() {
            self.objects.insert(hash, data.as_ref())?;
        }

        Ok(Object::from_hash(hash.try_into().unwrap()))
    }

    /// Serialize `value`, and store it as an [Object]
    fn serialize<T: Serialize>(&self, value: &T) -> anyhow::Result<Object> {
        let data = bincode::serialize(value)?;

        Ok(self.create_object(&data)?)
    }

    /// Load an [Object] from the database
    fn load(&self, object: &Object) -> sled::Result<sled::IVec> {
        self.objects.get(&object.hash()).map(|obj| obj.unwrap())
    }

    /// Load an [Object] and deserialize it
    fn deserialize<T: DeserializeOwned>(&self, object: &Object) -> anyhow::Result<T> {
        let data = self.load(object)?.to_vec();
        let value = bincode::deserialize(&data)?;
        Ok(value)
    }

    fn set_key(&self, path: Path, key: crypto::Key) -> anyhow::Result<()> {
        let object = self.serialize(&key)?;

        self.with_root_mut("keyring", |node| {
            node.insert(path, object)?;

            Ok(())
        })?;

        Ok(())
    }

    fn get_key(&self, path: Path) -> anyhow::Result<crypto::Key> {
        let object = self.with_root("keyring", Revision::FromLatest(0), |node| {
            let node = node.traverse(path)?.and_then(|node| node.file());

            Ok(node.cloned())
        })?;

        if let Some(object) = object {
            let key = self.deserialize(&object)?;

            Ok(key)
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, format!("key '{path}' not found")).into())
        }
    }
}

// public helpers
impl Files {
    /// Clears the files database
    pub fn clear(&self) -> anyhow::Result<()> {
        log::info!("clearing database");
        self.objects.clear()?;
        self.roots.clear()?;

        // create a new root node
        let dir = Node::new_dir();
        let object = self.serialize(&dir)?;
        self.roots.merge("fs", object.hash())?;

        Ok(())
    }

    pub fn insert(&self, path: Path, data: &[u8]) -> anyhow::Result<()> {
        log::debug!("inserting to file {path}");

        let object = self.create_object(data)?;
        let (parent, _) = path.parent_child();

        self.with_root_mut("fs", |node| {
            node.make_dir_recursive(parent)?;
            node.insert(path, object)?;
            Ok(())
        })?;

        log::debug!("inserted file '{path}' (object {})", object.hex());

        Ok(())
    }

    pub fn get(&self, path: Path, revision: Revision) -> anyhow::Result<Option<IVec>> {
        log::debug!("retrieving file {path}");

        let object = self.with_root("fs", revision, |node| {
            let node = node.traverse(path)?;
            let object = node.and_then(|node| node.file());

            Ok(object.cloned())
        })?;

        if let Some(object) = object {
            log::debug!("got object {} for file '{path}'", object.hex());

            Ok(Some(self.load(&object)?))
        } else {
            log::error!("file '{path}' not found");
            Ok(None)
        }
    }

    pub fn delete(&self, path: Path) -> anyhow::Result<()> {
        log::debug!("deleting file '{path}'");

        self.with_root_mut("fs", |node| {
            node.delete(path)?;

            Ok(())
        })?;

        Ok(())
    }

    pub fn rollback(&self, revision: Revision) -> anyhow::Result<()> {
        // the node to roll back to
        let mut old_root = self.get_root("fs", revision)?;

        self.set_root("fs", old_root)?;

        Ok(())
    }

    pub fn get_node(&self, path: Path, revision: Revision) -> anyhow::Result<Option<Node>> {
        log::debug!("retrieving node '{path}'");

        let node = self.with_root("fs", revision, |node| {
            let node = node.traverse(path)?;

            Ok(node.cloned())
        })?;

        Ok(node)
    }

    pub fn get_history(&self) -> anyhow::Result<RootHistory> {
        self.get_root_history("fs")
    }

    pub fn set_admin_key(&self, key: crypto::Key) -> anyhow::Result<()> {
        let path = Path::new("/self/admin")?;

        self.set_key(path, key)
    }

    pub fn get_admin_key(&self) -> anyhow::Result<crypto::Key> {
        let path = Path::new("/self/admin")?;

        self.get_key(path)
    }

    pub fn set_server_key(&self, key: crypto::Key) -> anyhow::Result<()> {
        let path = Path::new("/self/server")?;

        let admin_key = self.get_admin_key()?;

        if key.verify(&admin_key)? {
            self.set_key(path, key)?;
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "server key must be signed by admin key",
            )
            .into())
        }
    }

    pub fn get_server_key(&self) -> anyhow::Result<crypto::Key> {
        let path = Path::new("/self/server")?;

        let key = self.get_key(path)?;
        let admin_key = self.get_admin_key()?;

        if key.verify(&admin_key)? {
            Ok(key)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "server key must be signed by admin key",
            )
            .into())
        }
    }

    pub fn trust_client(&self, mut key: crypto::Key) -> anyhow::Result<()> {
        let path = Path::new("/trusted/client")?;

        let server_key = self.get_server_key()?;

        key.sign(&server_key)?;

        self.set_key(path, key)?;
        Ok(())
    }

    pub fn verify_client(&self, key: &crypto::Key) -> anyhow::Result<bool> {
        let server_key = self.get_server_key()?;

        let client_key = self.get_key(Path::new("/trusted/client")?)?;

        let is_verified = client_key.verify(&server_key)?;
        let is_equal = client_key.pub_key().raw() == key.pub_key().raw();

        Ok(is_verified && is_equal)
    }
}
