use std::path::Path;

use digest::Digest;

use common::Object;

pub struct Files {
    // _db: sled::Db,
    objects: sled::Tree,
    links: sled::Tree
}

impl Files {
    pub fn open(path: impl AsRef<Path>) -> sled::Result<Files> {
        let db = sled::open(path)?;
        let objects = db.open_tree("objects")?;
        let links = db.open_tree("links")?;

        Ok(Files {
            objects, links
        })
    }
    
    pub fn clear(&self) -> sled::Result<()> {
        self.objects.clear()?;
        self.links.clear()?;

        Ok(())
    }

    pub fn insert(&self, name: &str, data: impl AsRef<[u8]>) -> sled::Result<Object> {
        let mut hasher = sha2::Sha256::new();
        hasher.update(data.as_ref());
        let hash = hasher.finalize();

        if self.objects.get(hash)?.is_none() {
            self.objects.insert(hash, data.as_ref())?;
        }
        self.links.insert(name.as_bytes(), &hash[..])?;

        Ok(Object::from_hash(hash.try_into().unwrap()))
    }

    pub fn lookup(&self, name: &str) -> sled::Result<Option<Object>> {
        let hash = self.links.get(name.as_bytes())?
            .map(|hash| Object::from_hash((&hash[..]).try_into().expect("invalid hash")));

        Ok(hash)
    }

    pub fn get(&self, object: &Object) -> sled::Result<sled::IVec> {
        self.objects.get(&object.hash()).map(|obj| obj.unwrap())
    }

    pub fn objects(&self) -> impl Iterator<Item = sled::Result<(Object, sled::IVec)>> {
        self.objects.iter()
            .map(|r| {
                r.map(|(hash, data)| {
                    let hash = (&hash[..]).try_into().expect("invalid hash");
                    let object = Object::from_hash(hash);

                    (object, data)
                })
            })
    }

    pub fn links(&self) -> impl Iterator<Item = sled::Result<(String, Object)>> {
        self.links.iter()
            .map(|r| {
                r.map(|(name, hash)| {
                    let hash = (&hash[..]).try_into().expect("invalid hash");
                    let object = Object::from_hash(hash);

                    let name = String::from_utf8_lossy(&name[..]).into();

                    (name, object)
                })
            })
    }
}