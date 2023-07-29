use std::{collections::HashMap, io};
use std::path::Path as SysPath;

use digest::Digest;

use serde::{Serialize, Deserialize, de::DeserializeOwned};

use common::{Object, Path};

/// A [Node] represents a filesystem tree
#[derive(Serialize, Deserialize, Debug)]
pub enum Node {
    Dir(HashMap<String, Node>),
    File(Object)
}

impl Node {
    /// Create a new empty [Node::Dir]
    pub fn new_dir() -> Node {
        Node::Dir(HashMap::new())
    }

    /// Create a new [Node::File] referencing a given [Object] 
    pub fn new_file(object: Object) -> Node {
        Node::File(object)
    }

    /// Returns `Some(map)` if `self` is [Node::Dir]
    pub fn dir(&mut self) -> Option<&mut HashMap<String, Node>> {
        if let Node::Dir(map) = self {
            Some(map)
        } else {
            None
        }
    }

    /// Returns `Some(object)` if `self` is [Node::Dir]
    pub fn file(&mut self) -> Option<&mut Object> {
        if let Node::File(object) = self {
            Some(object)
        } else {
            None
        }
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

    // pub fn children(&mut self) -> io::Result<impl Iterator<Item = (&String, &mut Node)>> {
    //     if let Some(map) = self.dir() {
    //         Ok(map.iter_mut())
    //     } else {
    //         Err(io::ErrorKind::NotADirectory.into())
    //     }
    // }

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
        log::info!("opening objects and links trees");
        let objects = db.open_tree("objects")?;
        let roots = db.open_tree("roots")?;

        let files = Files {
            objects, roots
        };

        files.clear()?;

        let dir = Node::Dir(HashMap::new());
        let object = files.serialize(&dir)?;
        files.roots.insert("root", object.hash())?;

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
    pub fn clear(&self) -> sled::Result<()> {
        log::info!("clearing database");
        self.objects.clear()?;
        self.roots.clear()?;

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