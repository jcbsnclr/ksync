use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::io;
use std::ops::DerefMut;
use std::time::SystemTime;

use crate::files::{Object, Path};

/// A [Node] represents a filesystem tree
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NodeData {
    Dir(HashMap<String, Node>),
    File(Object),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Node {
    data: Option<NodeData>,
    timestamp: u128,
}

impl Node {
    pub fn new(data: NodeData) -> Node {
        Node {
            data: Some(data),
            timestamp: SystemTime::UNIX_EPOCH.elapsed().unwrap().as_nanos(),
        }
    }

    pub fn data(&mut self) -> &mut Option<NodeData> {
        &mut self.data
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
        if let Some(NodeData::Dir(map)) = &mut self.data {
            Some(map)
        } else {
            None
        }
    }

    /// Returns `Some(object)` if `self` is [Node::Dir]
    pub fn file(&mut self) -> Option<&mut Object> {
        if let Some(NodeData::File(object)) = &mut self.data {
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

    /// Returns a mutable reference to a given child. Will error if `self` is not a directory
    pub fn get_child(&mut self, name: &str) -> io::Result<Option<&mut Node>> {
        if let Some(map) = self.dir() {
            Ok(map.get_mut(&name.to_string()))
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
    pub fn traverse(&mut self, path: Path) -> io::Result<Option<&mut Node>> {
        if path.as_str() != "/" {
            let mut current = self;

            for part in path.parts() {
                current = if let Some(node) = current.get_child(&part)? {
                    node
                } else {
                    return Ok(None);
                }
            }

            Ok(Some(current))
        } else {
            Ok(Some(self))
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
            let node = self.traverse(path)?.ok_or(io::ErrorKind::NotFound)?;

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
            let node = self.traverse(path)?.ok_or(io::ErrorKind::NotFound)?;
            node.insert_child(name, Node::new_file(object))?;

            Ok(())
        } else {
            let err: io::Error = io::ErrorKind::InvalidFilename.into();
            Err(err.into())
        }
    }

    pub fn delete(&mut self, path: Path) -> io::Result<()> {
        let node = self.traverse(path)?.ok_or(io::ErrorKind::NotFound)?;

        *node.data() = None;

        Ok(())
    }

    pub fn file_list<'a>(&'a mut self) -> io::Result<FileList<'a>> {
        if self.dir().is_some() {
            Ok(FileList {
                node_stack: vec![("/".to_string(), self)],
                output_stack: vec![],
            })
        } else {
            Err(io::ErrorKind::NotADirectory.into())
        }
    }

    pub fn iter(&self) -> NodeIter {
        NodeIter {
            node_stack: vec![("".to_owned(), self.clone())],
            output_stack: vec![],
        }
    }
}

pub struct NodeIter {
    node_stack: Vec<(String, Node)>,
    output_stack: Vec<(String, Node)>,
}

impl Iterator for NodeIter {
    type Item = (String, Node);

    fn next(&mut self) -> Option<Self::Item> {
        if !self.output_stack.is_empty() {
            self.output_stack.pop()
        } else {
            if let Some((path, mut node)) = self.node_stack.pop() {
                if node.file().is_some() || node.data().is_none() {
                    Some((path, node))
                } else if let Some(map) = node.dir() {
                    for (name, node) in map {
                        self.node_stack
                            .push((path.clone() + "/" + name, node.clone()));
                    }

                    if !path.is_empty() {
                        Some((path, node))
                    } else {
                        Some(("/".to_owned(), node))
                    }
                } else {
                    unreachable!()
                }
            } else {
                None
            }
        }
    }
}

pub struct FileList<'a> {
    node_stack: Vec<(String, &'a mut Node)>,
    output_stack: Vec<(String, Object, u128)>,
}

impl<'a> FileList<'a> {
    pub fn as_map(self) -> HashMap<String, (Object, u128)> {
        self.map(|(path, object, timestamp)| (path, (object, timestamp)))
            .collect()
    }
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
                        Some(NodeData::Dir(_)) => {
                            self.node_stack.push((format!("{}{}/", path, name), node))
                        }

                        // if it is a file, push it to the output stack
                        Some(NodeData::File(object)) => self.output_stack.push((
                            format!("{}{}", path, name),
                            object,
                            node.timestamp(),
                        )),

                        None => continue,
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
