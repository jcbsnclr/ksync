use std::ops::DerefMut;
use std::time::SystemTime;
use std::{collections::HashMap, io};
use std::path::Path as SysPath;
use std::fmt::Debug;

use digest::Digest;

use serde::{Serialize, Deserialize, de::DeserializeOwned};

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
        let double_slashes = str.split('/')
            .skip(1)
            .filter(|&p| p == "")
            .count();

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
                Some(&self.0[last_slash + 1..]) // return path after last slash as the child
            )
        }
    }

    /// Returns an [Iterator] over a [Path]'s ancestors. See [Ancestors] for details
    pub fn ancestors(&'a self) -> Ancestors<'a> {
        Ancestors { path: *self, iter: 0 }
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
    iter: usize
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = Path<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = if self.iter != 0 {
            let index = self.path.0.char_indices()
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

/// A [Node] represents a filesystem tree
#[derive(Serialize, Deserialize, Debug)]
pub enum NodeData {
    Dir(HashMap<String, Node>),
    File(Object)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Node {
    data: NodeData,
    timestamp: u128
}

impl std::ops::Deref for Node {
    type Target = NodeData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for Node {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl Node {
    pub fn new(data: NodeData) -> Node {
        Node {
            data,
            timestamp: SystemTime::UNIX_EPOCH.elapsed().unwrap().as_nanos()
        }
    }

    /// Create a new empty [Node::Dir]
    pub fn new_dir() -> Node {
        Node::new(NodeData::Dir(HashMap::new()))
    }

    /// Create a new [Node::File] referencing a given [Object] 
    pub fn new_file(object: Object) -> Node {
        Node::new(NodeData::File(object))
    }

    /// Returns `Some(map)` if `self` is [Node::Dir]
    pub fn dir(&mut self) -> Option<&mut HashMap<String, Node>> {
        if let NodeData::Dir(map) = &mut self.data {
            Some(map)
        } else {
            None
        }
    }

    /// Returns `Some(object)` if `self` is [Node::Dir]
    pub fn file(&mut self) -> Option<&mut Object> {
        if let NodeData::File(object) = &mut self.data {
            Some(object)
        } else {
            None
        }
    }

    pub fn timestamp(&mut self) -> u128 {
        self.timestamp
    }

    /// Checks to see if a node contains a given child `name`
    pub fn has_child(&mut self, name: &str) -> io::Result<bool> {
        if let Some(map) = self.dir() {
            Ok(map.contains_key(&name.to_string()))
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    /// Returns a mutable reference to a given child. Will error if `self` is not a directory, or if the child is not found
    pub fn get_child(&mut self, name: &str) -> io::Result<&mut Node> {
        if let Some(map) = self.dir() {
            if let Some(child) = map.get_mut(&name.to_string()) {
                Ok(child)
            } else {
                Err(io::ErrorKind::NotFound.into())
            }
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    /// Inserts a child into `self`. If `self` is not [Node::Dir], then return an error
    pub fn insert_child(&mut self, name: &str, node: Node) -> io::Result<()> {
        if let Some(map) = self.dir() {
            map.insert(name.to_string(), node);

            Ok(())
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    /// Returns a mutable reference to a [Node] at a given [Path], relative to `self`
    pub fn traverse(&mut self, path: Path) -> io::Result<&mut Node> {
        if path.as_str() != "/" {
            let mut current = self;

            for part in path.parts() {
                current = current.get_child(&part)?;
            }

            Ok(current)
        } else {
            Ok(self)
        }
    }

    pub fn children(&mut self) -> io::Result<impl Iterator<Item = (&String, &mut Node)>> {
        if let Some(map) = self.dir() {
            Ok(map.iter_mut())
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    /// Make a directory at a given path relative to `self`. Will error if `self` is not a [Node::Dir], or if the parent of a given folder does not exist.
    pub fn make_dir(&mut self, path: Path) -> io::Result<()> {
        if let (path, Some(name)) = path.parent_child() {
            let node = self.traverse(path)?;

            if !node.has_child(name)? {
                node.insert_child(name, Node::new_dir())?;
            }
        }

        Ok(())
    }

    /// Recursively make new directories from a given [Path]
    pub fn make_dir_recursive(&mut self, path: Path) -> io::Result<()> {
        for ancestor in path.ancestors().skip(1) {
            self.make_dir(ancestor)?;
        }

        self.make_dir(path)?;

        Ok(())
    }

    /// Creates a new [Node::File] at a given [Path], referencing an [Object]
    pub fn insert(&mut self, path: Path, object: Object) -> io::Result<()> {
        if let (path, Some(name)) = path.parent_child() {
            // self.make_dir_recursive(path)?;
            let node = self.traverse(path)?;
            node.insert_child(name, Node::new_file(object))?;

            Ok(())
        } else {
            let err: io::Error = io::ErrorKind::InvalidFilename.into();
            Err(err.into())
        }
    }

    pub fn file_list<'a>(&'a mut self) -> io::Result<FileList<'a>> {
        if self.dir().is_some() {
            Ok(FileList {
                node_stack: vec![("/".to_string(), self)],
                output_stack: vec![]
            })
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }
}

pub struct FileList<'a> {
    node_stack: Vec<(String, &'a mut Node)>,
    output_stack: Vec<(String, Object, u128)>,
}

impl<'a> Iterator for FileList<'a> {
    type Item = (String, Object, u128);

    fn next(&mut self) -> Option<Self::Item> {
        if !self.output_stack.is_empty() {
            // return value from output queue
            self.output_stack.pop()
        } else {
            if let Some((path, node)) = self.node_stack.pop() {
                // iterate over children of next item in node stack
                for (name, node) in node.children().unwrap() {
                    match node.data {
                        // if it is a dir, push to the node stack to be processed later
                        NodeData::Dir(_) => self.node_stack.push((format!("{}{}/", path, name), node)),

                        // if it is a file, push it to the output stack 
                        NodeData::File(object) => self.output_stack.push((format!("{}{}", path, name), object.clone(), node.timestamp()))
                    }
                }

                // call next again once the current dir has been processed
                self.next()
            } else {
                // no more files to process
                None
            }
        }
    }
}

pub struct Files {
    // _db: sled::Db,
    /// A tree that maps an [Object] to it's data 
    objects: sled::Tree,
    /// A tree that maps a string "root" name, to an [Object] containing a filesystem [Node]
    roots: sled::Tree
}

impl Files {
    /// Opens a [Files] database from a given path, and initialises it
    // TODO: stop re-initialising the database on each open
    pub fn open(path: impl AsRef<SysPath>) -> anyhow::Result<Files> {
        log::info!("opening db at {:?}", path.as_ref());
        let db = sled::open(path)?;
        log::info!("opening objects and roots trees");
        let objects = db.open_tree("objects")?;
        let roots = db.open_tree("roots")?;

        let files = Files {
            objects, roots
        };

        // if root node does not exist, create it 
        if files.roots.get("root")?.is_none() {
            let dir = Node::new_dir();
            let object = files.serialize(&dir)?;
            files.roots.insert("root", object.hash())?;
        }

        Ok(files)
    }

    /// Perform operations on a given `root`
    pub fn with_root<T>(&self, root: &str, op: impl Fn(&mut Node) -> anyhow::Result<T>) -> anyhow::Result<T> {
        // load root node from database
        let hash = self.roots.get(root)?
            .ok_or(io::Error::new(io::ErrorKind::NotFound, "root not found"))?;
        let object = Object::from_hash((&hash[..]).try_into().unwrap());
        let mut node = self.deserialize(&object)?;

        // perform operation on node
        let result = op(&mut node)?;

        // re-serialize and store new root 
        let object = self.serialize(&node)?;
        self.roots.insert(root, object.hash())?;

        Ok(result)
    }

    /// Clears the files database
    pub fn clear(&self) -> anyhow::Result<()> {
        log::info!("clearing database");
        self.objects.clear()?;
        self.roots.clear()?;

        // create a new root node
        let dir = Node::new_dir();
        let object = self.serialize(&dir)?;
        self.roots.insert("root", object.hash())?;

        Ok(())
    }

    /// Create a new [Object] containing `data`, referenced by it's hash
    pub fn create_object(&self, data: impl AsRef<[u8]>) -> sled::Result<Object> {
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
    pub fn serialize<T: Serialize>(&self, value: &T) -> anyhow::Result<Object> {
        let data = bincode::serialize(value)?;
        
        Ok(self.create_object(&data)?)
    }

    /// Load an [Object] from the database
    pub fn get(&self, object: &Object) -> sled::Result<sled::IVec> {
        self.objects.get(&object.hash()).map(|obj| obj.unwrap())
    }

    /// Load an [Object] and deserialize it 
    pub fn deserialize<T: DeserializeOwned>(&self, object: &Object) -> anyhow::Result<T> {
        let data = self.get(object)?.to_vec();
        let value = bincode::deserialize(&data)?;
        Ok(value)
    }
}