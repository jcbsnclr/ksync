pub mod fmt;
pub mod proto;
pub mod util;

use std::fmt::Debug;

use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct Object([u8; 32]);

impl Object {
    pub fn from_hash(hash: [u8; 32]) -> Object {
        Object(hash)
    }

    pub fn hash(&self) -> &[u8; 32] {
        &self.0
    }

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

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(try_from = "&str")]
pub struct Path<'a>(&'a str);

#[derive(thiserror::Error, Debug)]
#[error("path must begin with a /, and must not contain double slashes")]
pub struct InvalidPath;

impl<'a> Path<'a> {
    pub fn new(str: &'a str) -> Result<Path<'a>, InvalidPath> {
        let double_slashes = str.split('/')
            .skip(1)
            .filter(|&p| p == "")
            .count();

        if !str.starts_with('/') || (double_slashes != 0 && str != "/") {
            Err(InvalidPath)
        } else {
            Ok(Path(str))
        }
    }

    pub fn as_str(&'a self) -> &'a str { 
        self.0
    }

    pub fn parts(&self) -> impl DoubleEndedIterator<Item = &'a str> {
        (&self.0[1..]).split('/')
    }

    pub fn parent_child(&'a self) -> (Path<'a>, Option<&'a str>) {
        if self.0 == "/" {
            (*self, None)
        } else {
            let last_slash = self.0.rfind('/').unwrap();

            let path = if last_slash == 0 {
                Path::new("/").unwrap()
            } else {
                Path::new(&self.0[0..last_slash]).unwrap()
            };

            (path, Some(&self.0[last_slash + 1..]))
        }
    }

    pub fn ancestors(&'a self) -> Ancestors<'a> {
        Ancestors { path: *self, iter: 0 }
    }
}

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