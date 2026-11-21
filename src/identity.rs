//! Application identities.
//!
//! TLS certs prove which server/domain was reached. This is the other half:
//! long-lived Ed25519 keys that prove *which user or device* is on the other
//! end. Keys are stored as raw 32-byte seeds with mode 0600.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::proto::{Id, Nonce, PubKey, SigAlg};

pub const IDENTITY_FILE: &str = "identity.key";

pub struct Identity {
    signing: SigningKey,
    public: PubKey,
    id: Id,
}

impl Identity {
    pub fn generate() -> Result<Self> {
        let seed = random_array::<32>().context("generating identity seed")?;
        Ok(Self::from_seed(seed))
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let public = PubKey(*signing.verifying_key().as_bytes());
        let id = identity_id(SigAlg::Ed25519, &public);
        Self {
            signing,
            public,
            id,
        }
    }

    /// Load an identity, generating one on first use.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }

        let identity = Self::generate()?;
        identity.save(path)?;
        Ok(identity)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;

        let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "{} must contain a raw 32-byte Ed25519 seed, found {} bytes",
                path.display(),
                bytes.len()
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("cannot set permissions on {}", path.display()))?;
        }

        Ok(Self::from_seed(seed))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, self.signing.as_bytes())
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn id(&self) -> Id {
        self.id
    }

    pub fn public(&self) -> PubKey {
        self.public
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing.sign(message)
    }

    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<()> {
        let key = VerifyingKey::from_bytes(&self.public.0).context("invalid public key")?;
        key.verify_strict(message, signature)
            .context("signature verification failed")
    }
}

pub fn identity_id(sig_alg: SigAlg, public: &PubKey) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(b"ksync-id-v1");
    hasher.update([u8::from(sig_alg)]);
    hasher.update(public.0);
    Id(hasher.finalize().into())
}

pub fn verify_identity(sig_alg: SigAlg, public: &PubKey, id: Id) -> Result<()> {
    let expected = identity_id(sig_alg, public);
    if expected != id {
        bail!("identity id does not match public key");
    }
    Ok(())
}

pub fn random_nonce() -> Result<Nonce> {
    Ok(Nonce(random_array::<32>()?))
}

pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|err| anyhow::anyhow!("reading system randomness: {err}"))?;
    Ok(bytes)
}

/// Load an allowlist file: one hex identity id per line, `#` starts a comment.
pub fn load_allowed_clients(path: &Path) -> Result<HashSet<Id>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut allowed = HashSet::new();

    for (index, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let bytes = hex::decode(line).with_context(|| {
            format!(
                "{}:{}: identity id is not valid hex",
                path.display(),
                index + 1
            )
        })?;

        let id: [u8; 32] = bytes.try_into().map_err(|_| {
            anyhow::anyhow!(
                "{}:{}: identity id must be 32 bytes",
                path.display(),
                index + 1
            )
        })?;

        allowed.insert(Id(id));
    }

    Ok(allowed)
}

/// Default identity location inside the data directory.
pub fn default_path(dir: &Path) -> PathBuf {
    dir.join(IDENTITY_FILE)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_round_trips_through_disk() {
        let path = std::env::temp_dir().join(format!(
            "ksync-id-{}-{}.key",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load(&path).unwrap();

        assert_eq!(first.id(), second.id());
        assert_eq!(first.public(), second.public());

        let message = b"hello";
        let signature = first.sign(message);
        second.verify(message, &signature).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn id_matches_public_key() {
        let identity = Identity::generate().unwrap();
        verify_identity(SigAlg::Ed25519, &identity.public(), identity.id()).unwrap();
    }

    #[test]
    fn allowlist_parses_comments_and_hex() {
        let path = std::env::temp_dir().join(format!(
            "ksync-allowed-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let id = "11".repeat(32);
        fs::write(&path, format!("# comment\n\n{id}  # device\n")).unwrap();

        let allowed = load_allowed_clients(&path).unwrap();
        assert_eq!(allowed.len(), 1);

        fs::remove_file(path).unwrap();
    }
}
