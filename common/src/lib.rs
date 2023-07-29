pub mod fmt;
pub mod proto;
pub mod util;

use std::fmt::Debug;

use serde::{Serialize, Deserialize};

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
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Hash)]
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