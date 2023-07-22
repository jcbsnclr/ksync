pub mod fmt;

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