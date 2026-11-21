//! Channel-bound mutual authentication.
//!
//! Both sides sign a deterministic transcript containing the negotiated
//! parameters, fresh nonces, both identities, and the TLS exporter output.
//! The exporter is what makes the signature unusable by a relay: it is
//! derived from this specific TLS session.

use anyhow::{Context, Result, bail};
use ed25519_dalek::Signature;
use rustls::{ClientConnection, ServerConnection};

use crate::{
    identity::{Identity, verify_identity},
    proto::{Features, Id, Nonce, ProtocolVersion, PubKey, SigAlg},
};

const EXPORTER_LABEL: &[u8] = b"EXPORTER-ksync-auth-v1";
const TRANSCRIPT_LABEL: &[u8] = b"AUTH-ksync-v1";

/// Which side of the handshake produced a signature.
///
/// This is not sent on the wire. It is derived from connection direction and
/// mixed into the signed bytes so a signature cannot be reflected back in the
/// other direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Role {
    Initiator = 0x01,
    Acceptor = 0x02,
}

pub struct Handshake {
    pub version: ProtocolVersion,
    pub features: Features,
    pub sig_alg: SigAlg,
    pub initiator_nonce: Nonce,
    pub acceptor_nonce: Nonce,
    pub initiator_id: Id,
    pub acceptor_id: Id,
    pub initiator_pub: PubKey,
    pub acceptor_pub: PubKey,
    pub exporter: [u8; 32],
}

impl Handshake {
    pub fn transcript(&self, role: Role) -> Vec<u8> {
        fn field(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            out.extend_from_slice(bytes);
        }

        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(TRANSCRIPT_LABEL);
        out.extend_from_slice(&self.version.0.to_be_bytes());
        out.extend_from_slice(&self.features.bits().to_be_bytes());
        out.push(self.sig_alg.into());
        out.push(role as u8);

        field(&mut out, &self.initiator_nonce.0);
        field(&mut out, &self.acceptor_nonce.0);
        field(&mut out, &self.initiator_pub.0);
        field(&mut out, &self.acceptor_pub.0);
        field(&mut out, &self.initiator_id.0);
        field(&mut out, &self.acceptor_id.0);
        field(&mut out, &self.exporter);

        out
    }

    pub fn sign(&self, identity: &Identity, role: Role) -> Result<Vec<u8>> {
        let (expected_id, expected_pub) = match role {
            Role::Initiator => (self.initiator_id, self.initiator_pub),
            Role::Acceptor => (self.acceptor_id, self.acceptor_pub),
        };

        if identity.id() != expected_id || identity.public() != expected_pub {
            bail!("identity does not match the transcript role");
        }

        Ok(identity.sign(&self.transcript(role)).to_bytes().to_vec())
    }

    pub fn verify(&self, signer: Role, signature: &[u8]) -> Result<()> {
        if signature.len() != self.sig_alg.signature_len() {
            bail!("invalid signature length for {:?}", self.sig_alg);
        }

        let (public, id) = match signer {
            Role::Initiator => (self.initiator_pub, self.initiator_id),
            Role::Acceptor => (self.acceptor_pub, self.acceptor_id),
        };

        verify_identity(self.sig_alg, &public, id)?;

        let key = ed25519_dalek::VerifyingKey::from_bytes(&public.0)
            .context("invalid verification key")?;
        let signature = Signature::from_slice(signature).context("malformed signature")?;

        key.verify_strict(&self.transcript(signer), &signature)
            .context("signature verification failed")
    }
}

pub fn server_exporter(conn: &ServerConnection, version: ProtocolVersion) -> Result<[u8; 32]> {
    let context = version.0.to_be_bytes();
    conn.export_keying_material([0u8; 32], EXPORTER_LABEL, Some(&context))
        .context("TLS exporter failed")
}

pub fn client_exporter(conn: &ClientConnection, version: ProtocolVersion) -> Result<[u8; 32]> {
    let context = version.0.to_be_bytes();
    conn.export_keying_material([0u8; 32], EXPORTER_LABEL, Some(&context))
        .context("TLS exporter failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Handshake {
        let initiator = Identity::from_seed([1; 32]);
        let acceptor = Identity::from_seed([2; 32]);

        Handshake {
            version: ProtocolVersion(1),
            features: Features::NONE,
            sig_alg: SigAlg::Ed25519,
            initiator_nonce: Nonce([3; 32]),
            acceptor_nonce: Nonce([4; 32]),
            initiator_id: initiator.id(),
            acceptor_id: acceptor.id(),
            initiator_pub: initiator.public(),
            acceptor_pub: acceptor.public(),
            exporter: [5; 32],
        }
    }

    #[test]
    fn signatures_verify_in_the_right_direction() {
        let handshake = sample();
        let initiator = Identity::from_seed([1; 32]);
        let acceptor = Identity::from_seed([2; 32]);

        let client_signature = handshake.sign(&initiator, Role::Initiator).unwrap();
        handshake
            .verify(Role::Initiator, &client_signature)
            .unwrap();

        let server_signature = handshake.sign(&acceptor, Role::Acceptor).unwrap();
        handshake.verify(Role::Acceptor, &server_signature).unwrap();
    }

    #[test]
    fn a_signature_cannot_be_reflected_in_the_other_direction() {
        let handshake = sample();
        let initiator = Identity::from_seed([1; 32]);

        let signature = handshake.sign(&initiator, Role::Initiator).unwrap();
        assert!(handshake.verify(Role::Acceptor, &signature).is_err());
    }

    #[test]
    fn a_changed_exporter_invalidates_the_signature() {
        let handshake = sample();
        let initiator = Identity::from_seed([1; 32]);
        let signature = handshake.sign(&initiator, Role::Initiator).unwrap();

        let mut other = sample();
        other.exporter = [6; 32];

        assert!(other.verify(Role::Initiator, &signature).is_err());
    }

    #[test]
    fn the_transcript_is_deterministic() {
        assert_eq!(
            sample().transcript(Role::Initiator),
            sample().transcript(Role::Initiator)
        );
        assert_ne!(
            sample().transcript(Role::Initiator),
            sample().transcript(Role::Acceptor)
        );
    }
}
