use std::{collections::HashMap, io};
use std::path::Path as SysPath;

use digest::Digest;

use serde::{Serialize, Deserialize, de::DeserializeOwned};

use common::{Object, Path};

#[derive(Serialize, Deserialize, Debug)]
pub enum Node {
    Dir(HashMap<String, Node>),
    File(Object)
}

impl Node {
    pub fn new_dir() -> Node {
        Node::Dir(HashMap::new())
    }

    pub fn new_file(object: Object) -> Node {
        Node::File(object)
    }

    pub fn dir(&mut self) -> Option<&mut HashMap<String, Node>> {
        if let Node::Dir(map) = self {
            Some(map)
        } else {
            None
        }
    }

    pub fn file(&mut self) -> Option<&mut Object> {
        if let Node::File(object) = self {
            Some(object)
        } else {
            None
        }
    }

    pub fn has_child(&mut self, name: &str) -> io::Result<bool> {
        if let Some(map) = self.dir() {
            Ok(map.contains_key(&name.to_string()))
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

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

    pub fn insert_child(&mut self, name: &str, node: Node) -> io::Result<()> {
        if let Some(map) = self.dir() {
            map.insert(name.to_string(), node);

            Ok(())
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

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

    pub fn make_dir(&mut self, path: Path) -> io::Result<()> {
        if let (path, Some(name)) = path.parent_child() {
            let node = self.traverse(path)?;

            if !node.has_child(name)? {
                node.insert_child(name, Node::new_dir())?;
            }
        }

        Ok(())
    }

    pub fn make_dir_recursive(&mut self, path: Path) -> io::Result<()> {
        for ancestor in path.ancestors().skip(1) {
            self.make_dir(ancestor)?;
        }

        self.make_dir(path)?;

        Ok(())
    }

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
    objects: sled::Tree,
    links: sled::Tree,
    roots: sled::Tree
}

impl Files {
    pub fn open(path: impl AsRef<SysPath>) -> anyhow::Result<Files> {
        log::info!("opening db at {:?}", path.as_ref());
        let db = sled::open(path)?;
        log::info!("opening objects and links trees");
        let objects = db.open_tree("objects")?;
        let links = db.open_tree("links")?;
        let roots = db.open_tree("roots")?;

        let files = Files {
            objects, links, roots
        };

        files.clear()?;

        let dir = Node::Dir(HashMap::new());
        let object = files.serialize(&dir)?;
        files.roots.insert("root", object.hash())?;

        Ok(files)
    }

    pub fn with_root<T>(&self, root: &str, op: impl Fn(&mut Node) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let hash = self.roots.get(root)?
            .ok_or(io::Error::new(io::ErrorKind::NotFound, "root not found"))?;
        let object = Object::from_hash((&hash[..]).try_into().unwrap());
        let mut node = self.deserialize(&object)?;

        let result = op(&mut node)?;

        let object = self.serialize(&node)?;
        self.roots.insert(root, object.hash())?;

        Ok(result)
    }

    pub fn clear(&self) -> sled::Result<()> {
        log::info!("clearing database");
        self.objects.clear()?;
        self.links.clear()?;
        self.roots.clear()?;

        Ok(())
    }

    /// Create a new [Object] containing `data`, referenced by it's hash
    pub fn create_object(&self, data: impl AsRef<[u8]>) -> sled::Result<Object> {
        let mut hasher = sha2::Sha256::new();
        hasher.update(data.as_ref());
        let hash = hasher.finalize();

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

    // pub fn lookup(&self, path: Path) -> anyhow::Result<Option<Object>> {
    //     log::info!("looking up file {}", path);

    //     let mut root = self.get_root()?;
    //     let node = root.traverse(path)?;

    //     if let Some(object) = node.file().cloned() {
    //         log::info!("got object {}", object.hex());

    //         Ok(Some(object))
    //     } else {
    //         Ok(None)
    //     }
    // }

    // pub fn insert(&self, path: Path, data: impl AsRef<[u8]>) -> anyhow::Result<Object> {
    //     log::info!("inserting file {path}");
        
    //     let object = self.create_object(data.as_ref())?;
    //     let mut root = self.get_root()?;
        
    //     if let (path, Some(name)) = path.parent_child() {
    //         // self.make_dir_recursive(path)?;
    //         let node = root.traverse(path)?;
    //         node.insert_child(name, Node::new_file(object))?;

    //         self.set_root(root)?;
    //         Ok(object)
    //     } else {
    //         let err: io::Error = io::ErrorKind::InvalidFilename.into();
    //         Err(err.into())
    //     }
    // }

    // pub fn objects(&self) -> impl Iterator<Item = sled::Result<(Object, sled::IVec)>> {
    //     self.objects.iter()
    //         .map(|r| {
    //             r.map(|(hash, data)| {
    //                 let hash = (&hash[..]).try_into().expect("invalid hash");
    //                 let object = Object::from_hash(hash);

    //                 (object, data)
    //             })
    //         })
    // }

    // pub fn links(&self) -> impl Iterator<Item = sled::Result<(String, Object)>> {
    //     self.links.iter()
    //         .map(|r| {
    //             r.map(|(name, hash)| {
    //                 let hash = (&hash[..]).try_into().expect("invalid hash");
    //                 let object = Object::from_hash(hash);

    //                 let name = String::from_utf8_lossy(&name[..]).into();

    //                 (name, object)
    //             })
    //         })
    // }
}