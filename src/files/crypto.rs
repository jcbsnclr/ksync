use serde::{Serialize, Deserialize};

use ring::signature::{self, Ed25519KeyPair, UnparsedPublicKey, KeyPair};

use std::io;

#[derive(Debug, Serialize, Deserialize)]
pub enum KeyKind {
    Public(Vec<u8>),
    Pair(Vec<u8>)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Key {
    kind: KeyKind,
    identifier: String,
    signature: Option<Vec<u8>>
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            KeyKind::Pair(_) => write!(f, "public/private key pair ")?,
            KeyKind::Public(_) => write!(f, "public key ")?
        }

        write!(f, "'{}'", self.identifier)?;

        if let Some(signature) = &self.signature {
            write!(f, ", signature: {}", crate::util::fmt::HexSlice::from(&signature[..]))?;
        }

        Ok(())
    }
}

impl Key {
    pub fn from_key_pair(pair: &[u8], identifier: &str) -> anyhow::Result<Key> {
        Ed25519KeyPair::from_pkcs8(pair)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid key-pair"))?;

        Ok(Key {
            kind: KeyKind::Pair(pair.into()),
            identifier: identifier.to_owned(),
            signature: None
        })
    }
    
    // pub fn from_pub_key(pub_key: &[u8], identifier: &str) -> anyhow::Result<Key> {
    //     Ok(Key {
    //         kind: KeyKind::Public(pub_key.into()),
    //         identifier: identifier.into(),
    //         signature: None
    //     })
    // }

    pub fn sign(&mut self, with: &Key) -> anyhow::Result<()> {
        let bytes = bincode::serialize(&self.kind)?;
        
        match &with.kind {
            KeyKind::Pair(key) => {
                let pair = Ed25519KeyPair::from_pkcs8(&key)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "signing key must be a public/private key pair"))?;

                let signature = pair.sign(&bytes);

                self.signature = Some(signature.as_ref().to_owned());
                Ok(())
            },

            KeyKind::Public(_) => {
                let err: io::Error = io::ErrorKind::InvalidData.into();
                Err(err.into())
            }
        }
    }

    pub fn verify(&self, with: &Key) -> anyhow::Result<bool> {
        let pub_key = with.pub_key();
        let pub_key = UnparsedPublicKey::new(&signature::ED25519, pub_key.raw());

        match &self.signature {
            Some(signature) => {
                let data = bincode::serialize(&self.kind)?;

                Ok(pub_key.verify(&data, &signature).is_ok())
            },

            None => {
                Ok(false)
            }
        }
    }

    pub fn pub_key(&self) -> Key {
        let data = match &self.kind {
            KeyKind::Public(key) => key.clone(),
            KeyKind::Pair(key) => {
                let pair = Ed25519KeyPair::from_pkcs8(&key).unwrap();
                let key = pair.public_key();

                key.as_ref().to_owned()
            }
        };

        Key {
            kind: KeyKind::Public(data),
            identifier: self.identifier.clone(),
            signature: None
        }
    }

    pub fn raw<'a>(&'a self) -> &'a [u8] {
        match &self.kind {
            KeyKind::Public(data) | KeyKind::Pair(data) => &data[..]
        }
    }

    // pub fn key_pair(&self) -> Option<Ed25519KeyPair> {
    //     match &self.kind {
    //         KeyKind::Public(_) => None,
    //         KeyKind::Pair(key) => {
    //             let pair = Ed25519KeyPair::from_pkcs8(&key).unwrap();

    //             Some(pair)
    //         }
    //     }
    // }
}