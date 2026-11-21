//! Client-side handshake and request helpers.
//!
//! This is the mirror of the server's state machine. It is used by the
//! integration tests today and will back the eventual `ksync client` command.

use anyhow::{Context, Result, bail};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::{
    auth::{self, Handshake, Role},
    identity::{self, Identity},
    proto::{
        ALPN, Auth, Features, Hello, Id, MAX_AUTH, MAX_FRAME, MAX_HELLO, Message, PROTOCOL_VERSION,
        Request, RequestBody, RequestId, ResponseBody, SigAlg,
    },
};

/// Complete the version and authentication handshake.
///
/// `server_id` is the identity the caller expects. It is compared against
/// `HelloAck`; there is no trust-on-first-use here.
pub async fn handshake(
    mut tls: TlsStream<TcpStream>,
    identity: &Identity,
    server_id: Id,
    offered: Features,
) -> Result<TlsStream<TcpStream>> {
    if tls.get_ref().1.alpn_protocol() != Some(ALPN) {
        bail!("server did not negotiate the ksync ALPN protocol");
    }

    let initiator_nonce = identity::random_nonce()?;
    let hello = Message::Hello(Hello {
        version_min: PROTOCOL_VERSION,
        version_max: PROTOCOL_VERSION,
        sig_alg: SigAlg::Ed25519,
        features: offered,
        initiator_nonce,
        initiator_id: identity.id(),
        initiator_pub: identity.public(),
        acceptor_id: server_id,
    });

    crate::proto::write_typed(&mut tls, &hello).await?;

    let ack = match crate::proto::read_typed(&mut tls, MAX_HELLO).await? {
        Message::HelloAck(ack) => ack,
        Message::Error(err) => {
            bail!(
                "server rejected handshake ({:?}): {}",
                err.code,
                err.message()
            )
        }
        other => bail!("expected HelloAck, got {other:?}"),
    };

    check_ack(&ack, identity, server_id, offered)?;

    let exporter = auth::client_exporter(tls.get_ref().1, ack.version)?;

    let handshake = Handshake {
        version: ack.version,
        features: ack.features,
        sig_alg: ack.sig_alg,
        initiator_nonce,
        acceptor_nonce: ack.acceptor_nonce,
        initiator_id: identity.id(),
        acceptor_id: ack.acceptor_id,
        initiator_pub: identity.public(),
        acceptor_pub: ack.acceptor_pub,
        exporter,
    };

    let signature = handshake.sign(identity, Role::Initiator)?;
    crate::proto::write_typed(
        &mut tls,
        &Message::Auth(Auth {
            sig_alg: handshake.sig_alg,
            signature,
        }),
    )
    .await?;

    match crate::proto::read_typed(&mut tls, MAX_AUTH).await? {
        Message::AuthOk(ok) => handshake.verify(Role::Acceptor, &ok.signature)?,
        Message::Error(err) => {
            bail!(
                "server rejected authentication ({:?}): {}",
                err.code,
                err.message()
            )
        }
        other => bail!("expected AuthOk, got {other:?}"),
    }

    Ok(tls)
}

fn check_ack(
    ack: &crate::proto::HelloAck,
    identity: &Identity,
    server_id: Id,
    offered: Features,
) -> Result<()> {
    if ack.version != PROTOCOL_VERSION {
        bail!("server selected unsupported version {:?}", ack.version);
    }
    if ack.sig_alg != SigAlg::Ed25519 {
        bail!("server selected unsupported signature algorithm");
    }
    if ack.acceptor_id != server_id {
        bail!("server identity does not match the expected id");
    }
    if ack.initiator_id != identity.id() {
        bail!("server echoed the wrong initiator id");
    }
    if !ack.features.is_subset_of(offered) {
        bail!("server selected features that were not offered");
    }

    identity::verify_identity(ack.sig_alg, &ack.acceptor_pub, ack.acceptor_id)
        .context("server id does not match its public key")?;

    Ok(())
}

/// Send one request and return its response body.
pub async fn request(
    tls: &mut TlsStream<TcpStream>,
    id: RequestId,
    body: RequestBody,
) -> Result<ResponseBody> {
    crate::proto::write_typed(tls, &Message::Request(Request { id, body })).await?;

    match crate::proto::read_typed(tls, MAX_FRAME).await? {
        Message::Response(response) if response.id == id => Ok(response.body),
        Message::Response(response) => {
            bail!("response id {} does not match {}", response.id.0, id.0)
        }
        Message::Error(err) => bail!("server closed with {:?}: {}", err.code, err.message()),
        other => bail!("expected Response, got {other:?}"),
    }
}
