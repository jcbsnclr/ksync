//! Client-side handshake and request helpers.
//!
//! This is the mirror of the server's state machine. It is used by the
//! integration tests today and will back the eventual `ksync client` command.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustls::pki_types::ServerName;
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::{TlsConnector, client::TlsStream};

use crate::{
    auth::{self, Handshake, Role},
    config::ServerProfile,
    identity::{self, Identity},
    method::Method,
    proto::{
        ALPN, Auth, Features, Hello, Id, MAX_AUTH, MAX_FRAME, MAX_HELLO, Message, MethodName,
        PROTOCOL_VERSION, Request, RequestId, ResponseBody, SigAlg,
    },
    tls,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect, verify the server, and complete application authentication.
pub async fn connect(profile: &ServerProfile, identity: &Identity) -> Result<TlsStream<TcpStream>> {
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(profile.addr))
        .await
        .context("TCP connect timed out")??;
    tcp.set_nodelay(true)?;

    let config = tls::client_config(profile.ca.as_deref())?;
    let server_name =
        ServerName::try_from(profile.server_name.clone()).context("invalid TLS server name")?;

    let tls = timeout(
        CONNECT_TIMEOUT,
        TlsConnector::from(config).connect(server_name, tcp),
    )
    .await
    .context("TLS handshake timed out")??;

    timeout(
        AUTH_TIMEOUT,
        handshake(tls, identity, profile.server_id, Features::NONE),
    )
    .await
    .context("application authentication timed out")?
}

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

/// Invoke a method under the `core` namespace.
pub async fn call_core<M: Method>(
    tls: &mut TlsStream<TcpStream>,
    id: RequestId,
    input: M::Input,
) -> Result<M::Output> {
    call::<M>(tls, id, MethodName::core(M::NAME)?, input).await
}

/// Send one request and decode its typed response.
pub async fn call<M: Method>(
    tls: &mut TlsStream<TcpStream>,
    id: RequestId,
    method: MethodName,
    input: M::Input,
) -> Result<M::Output> {
    let payload = postcard::to_stdvec(&input)?;

    crate::proto::write_typed(
        tls,
        &Message::Request(Request {
            id,
            method,
            payload,
        }),
    )
    .await?;

    match crate::proto::read_typed(tls, MAX_FRAME).await? {
        Message::Response(response) if response.id == id => match response.body {
            ResponseBody::Ok(bytes) => {
                let (output, trailing) = postcard::take_from_bytes(&bytes)?;
                if !trailing.is_empty() {
                    bail!("trailing bytes in method response");
                }
                Ok(output)
            }
            ResponseBody::Error(err) => Err(method_error(err)),
        },
        Message::Response(response) => {
            bail!("response id {} does not match {}", response.id.0, id.0)
        }
        Message::Error(err) => bail!("server closed with {:?}: {}", err.code, err.message()),
        other => bail!("expected Response, got {other:?}"),
    }
}

fn method_error(err: crate::proto::WireError) -> anyhow::Error {
    anyhow::anyhow!("method failed ({:?}): {}", err.code, err.message())
}
