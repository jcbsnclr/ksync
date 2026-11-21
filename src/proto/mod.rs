//! Wire protocol: framing plus the postcard-encoded message schema.
//!
//! The schema is append-only. Variant order is the wire format, so new
//! variants go at the end and existing variants are never reordered,
//! removed, or reused.

pub mod frame;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub use frame::{read_frame, write_frame};

pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
pub const ALPN: &[u8] = b"ksync/1";

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const MAX_HELLO: usize = 4 * 1024;
pub const MAX_AUTH: usize = 8 * 1024;
pub const MAX_DETAIL: usize = 1024;

/// Ordered protocol version. Newtypes keep it from being confused with other
/// `u16` fields, while preserving numeric comparison for negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u16);

/// Stable, public fingerprint of an Ed25519 key. Not secret; used for
/// allowlists and for naming a peer in the auth transcript.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub [u8; 32]);

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Per-connection randomness. Not secret, but fresh for every handshake.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Nonce(pub [u8; 32]);

impl fmt::Debug for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PubKey(pub [u8; 32]);

impl fmt::Debug for PubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Opaque request identifier. Zero is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

/// Independent, optional capabilities. A set, not a choice; negotiated by
/// intersection. Unknown bits are ignored so new clients can talk to old
/// servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Features(pub u64);

impl Features {
    pub const NONE: Self = Self(0);

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }
}

/// Signature algorithm. Closed set with explicit numeric wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SigAlg {
    Ed25519 = 0,
}

impl SigAlg {
    pub const fn signature_len(self) -> usize {
        match self {
            Self::Ed25519 => 64,
        }
    }
}

impl TryFrom<u8> for SigAlg {
    type Error = UnknownSigAlg;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ed25519),
            other => Err(UnknownSigAlg(other)),
        }
    }
}

impl From<SigAlg> for u8 {
    fn from(alg: SigAlg) -> Self {
        alg as u8
    }
}

impl Serialize for SigAlg {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for SigAlg {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u8::deserialize(deserializer)?;
        Self::try_from(value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownSigAlg(pub u8);

impl fmt::Display for UnknownSigAlg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown signature algorithm {}", self.0)
    }
}

impl std::error::Error for UnknownSigAlg {}

/// Terminal protocol errors. Numeric values are part of the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Protocol,
    VersionMismatch,
    AuthFailed,
    Unauthenticated,
    InvalidRequest,
    UnknownRequest,
    NotFound,
    Conflict,
    TooLarge,
    RateLimited,
    Internal,
    Shutdown,
    /// Forward-compatible carrier for a code this build does not know.
    Unknown(u16),
}

impl From<u16> for ErrorCode {
    fn from(value: u16) -> Self {
        match value {
            0 => Self::Protocol,
            1 => Self::VersionMismatch,
            2 => Self::AuthFailed,
            3 => Self::Unauthenticated,
            4 => Self::InvalidRequest,
            5 => Self::UnknownRequest,
            6 => Self::NotFound,
            7 => Self::Conflict,
            8 => Self::TooLarge,
            9 => Self::RateLimited,
            10 => Self::Internal,
            11 => Self::Shutdown,
            other => Self::Unknown(other),
        }
    }
}

impl From<ErrorCode> for u16 {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::Protocol => 0,
            ErrorCode::VersionMismatch => 1,
            ErrorCode::AuthFailed => 2,
            ErrorCode::Unauthenticated => 3,
            ErrorCode::InvalidRequest => 4,
            ErrorCode::UnknownRequest => 5,
            ErrorCode::NotFound => 6,
            ErrorCode::Conflict => 7,
            ErrorCode::TooLarge => 8,
            ErrorCode::RateLimited => 9,
            ErrorCode::Internal => 10,
            ErrorCode::Shutdown => 11,
            ErrorCode::Unknown(other) => other,
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16((*self).into())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(u16::deserialize(deserializer)?.into())
    }
}

/// Top-level message. Variant order is frozen; append only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    Hello(Hello),       // 0
    HelloAck(HelloAck), // 1
    Auth(Auth),         // 2
    AuthOk(AuthOk),     // 3
    Request(Request),   // 4
    Response(Response), // 5
    Error(WireError),   // 6
                        // 7 reserved for Event
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version_min: ProtocolVersion,
    pub version_max: ProtocolVersion,
    pub sig_alg: SigAlg,
    pub features: Features,
    pub initiator_nonce: Nonce,
    pub initiator_id: Id,
    pub initiator_pub: PubKey,
    pub acceptor_id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub version: ProtocolVersion,
    pub sig_alg: SigAlg,
    pub features: Features,
    pub acceptor_nonce: Nonce,
    pub acceptor_id: Id,
    pub acceptor_pub: PubKey,
    pub initiator_id: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    pub sig_alg: SigAlg,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthOk {
    pub sig_alg: SigAlg,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub body: RequestBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RequestBody {
    Ping, // 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: RequestId,
    pub body: ResponseBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseBody {
    Pong,             // 0
    Error(WireError), // 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireError {
    pub code: ErrorCode,
    #[serde(with = "serde_bytes")]
    pub detail: Vec<u8>,
}

impl WireError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        let mut detail = detail.into().into_bytes();
        detail.truncate(MAX_DETAIL);
        Self { code, detail }
    }

    pub fn message(&self) -> String {
        String::from_utf8_lossy(&self.detail).into_owned()
    }
}

pub fn encode(message: &Message) -> anyhow::Result<Vec<u8>> {
    let body = postcard::to_stdvec(message)?;
    if body.len() > MAX_FRAME {
        anyhow::bail!("encoded message exceeds {MAX_FRAME} bytes");
    }
    Ok(body)
}

pub fn decode(body: &[u8]) -> anyhow::Result<Message> {
    if body.is_empty() {
        anyhow::bail!("empty message");
    }
    let (message, trailing) = postcard::take_from_bytes(body)?;
    if !trailing.is_empty() {
        anyhow::bail!("trailing bytes after message");
    }
    Ok(message)
}

/// Read a frame and decode it, rejecting messages above a per-type limit.
pub async fn read_typed<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> anyhow::Result<Message> {
    let body = read_frame(reader).await?;
    if body.len() > max {
        anyhow::bail!("message exceeds {max} bytes");
    }
    decode(&body)
}

pub async fn write_typed<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Message,
) -> anyhow::Result<()> {
    let body = encode(message)?;
    write_frame(writer, &body).await
}

/// Negotiate the highest version supported by both sides. This build only
/// supports one version, but the range is part of the protocol.
pub fn negotiate(
    client_min: ProtocolVersion,
    client_max: ProtocolVersion,
) -> Option<ProtocolVersion> {
    if client_min <= PROTOCOL_VERSION && PROTOCOL_VERSION <= client_max {
        Some(PROTOCOL_VERSION)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Message {
        Message::Hello(Hello {
            version_min: PROTOCOL_VERSION,
            version_max: PROTOCOL_VERSION,
            sig_alg: SigAlg::Ed25519,
            features: Features::NONE,
            initiator_nonce: Nonce([1; 32]),
            initiator_id: Id([2; 32]),
            initiator_pub: PubKey([3; 32]),
            acceptor_id: Id([4; 32]),
        })
    }

    #[test]
    fn message_round_trips() {
        let encoded = encode(&hello()).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert!(matches!(decoded, Message::Hello(_)));
    }

    #[test]
    fn rejects_empty_and_trailing_bytes() {
        assert!(decode(&[]).is_err());

        let mut encoded = encode(&hello()).unwrap();
        encoded.push(0);
        assert!(decode(&encoded).is_err());
    }

    #[test]
    fn variant_indices_are_frozen() {
        // Postcard encodes the variant index as a varint. These values are a
        // compatibility promise: reordering `Message` must fail this test.
        let hello = encode(&hello()).unwrap();
        assert_eq!(hello[0], 0);

        let error = encode(&Message::Error(WireError::new(ErrorCode::Protocol, "x"))).unwrap();
        assert_eq!(error[0], 6);
    }

    #[test]
    fn error_codes_round_trip_as_numbers() {
        for code in [0u16, 1, 2, 11, 999] {
            let encoded = postcard::to_stdvec(&ErrorCode::from(code)).unwrap();
            assert_eq!(encoded, postcard::to_stdvec(&code).unwrap());
            let decoded: ErrorCode = postcard::from_bytes(&encoded).unwrap();
            assert_eq!(u16::from(decoded), code);
        }
    }

    #[test]
    fn sig_alg_round_trips_as_a_number() {
        let encoded = postcard::to_stdvec(&SigAlg::Ed25519).unwrap();
        assert_eq!(encoded, vec![0]);
        let decoded: SigAlg = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, SigAlg::Ed25519);
    }

    #[test]
    fn version_negotiation() {
        assert_eq!(
            negotiate(ProtocolVersion(1), ProtocolVersion(1)),
            Some(PROTOCOL_VERSION)
        );
        assert_eq!(negotiate(ProtocolVersion(2), ProtocolVersion(3)), None);
    }
}
