//! Golden wire vectors.
//!
//! These are known-answer tests: the exact bytes are part of the compatibility
//! contract. Changing the schema should require updating a vector in the same
//! commit, ideally with a protocol version bump.

use ksync::{
    auth::{Handshake, Role},
    identity::Identity,
    proto::{
        Auth, AuthOk, ErrorCode, Features, Hello, HelloAck, Id, Message, MethodName, Nonce,
        PROTOCOL_VERSION, ProtocolVersion, PubKey, Request, RequestId, Response, ResponseBody,
        SigAlg, WireError,
    },
};

const HELLO: &str = "00010100000101010101010101010101010101010101010101010101010101010101010101020202020202020202020202020202020202020202020202020202020202020203030303030303030303030303030303030303030303030303030303030303030404040404040404040404040404040404040404040404040404040404040404";

const HELLO_ACK: &str = "010100000505050505050505050505050505050505050505050505050505050505050505040404040404040404040404040404040404040404040404040404040404040406060606060606060606060606060606060606060606060606060606060606060202020202020202020202020202020202020202020202020202020202020202";

const AUTH: &str = "020040aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const AUTH_OK: &str = "030040bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const REQUEST: &str = "040709636f72653a70696e6700";

const REQUEST_WITH_PAYLOAD: &str = "040709636f72653a70696e6703010203";

const RESPONSE_OK: &str = "05070000";

const RESPONSE_OK_WITH_PAYLOAD: &str = "050700020908";

const RESPONSE_ERROR: &str = "05070105046e6f7065";

const ERROR: &str = "060003626164";

const TRANSCRIPT_INITIATOR: &str = "415554482d6b73796e632d7631000100000000000000000001000000200303030303030303030303030303030303030303030303030303030303030303000000200404040404040404040404040404040404040404040404040404040404040404000000208a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c000000208139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3940000002049baea4ab9c843430c4cc2bf1f7291a27893536bbde23bddc33c326fdd1ecf38000000203dd662d028f9044b4a3dc729f4f146ef47691fecdcd816afba4301aaaba1846f000000200505050505050505050505050505050505050505050505050505050505050505";

const TRANSCRIPT_ACCEPTOR: &str = "415554482d6b73796e632d7631000100000000000000000002000000200303030303030303030303030303030303030303030303030303030303030303000000200404040404040404040404040404040404040404040404040404040404040404000000208a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c000000208139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3940000002049baea4ab9c843430c4cc2bf1f7291a27893536bbde23bddc33c326fdd1ecf38000000203dd662d028f9044b4a3dc729f4f146ef47691fecdcd816afba4301aaaba1846f000000200505050505050505050505050505050505050505050505050505050505050505";

const SIGNATURE_INITIATOR: &str = "3d7b2795ce3500bebb9b7c57e90767555101ab006d1d8d5c982adf99abb6b76884c15943bd03d3d9fd0d1f2843b63be05d9d913ac422904beec3ca321eb8dc06";

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

fn hello_ack() -> Message {
    Message::HelloAck(HelloAck {
        version: PROTOCOL_VERSION,
        sig_alg: SigAlg::Ed25519,
        features: Features::NONE,
        acceptor_nonce: Nonce([5; 32]),
        acceptor_id: Id([4; 32]),
        acceptor_pub: PubKey([6; 32]),
        initiator_id: Id([2; 32]),
    })
}

fn auth() -> Message {
    Message::Auth(Auth {
        sig_alg: SigAlg::Ed25519,
        signature: vec![0xaa; 64],
    })
}

fn auth_ok() -> Message {
    Message::AuthOk(AuthOk {
        sig_alg: SigAlg::Ed25519,
        signature: vec![0xbb; 64],
    })
}

fn request() -> Message {
    Message::Request(Request {
        id: RequestId(7),
        method: MethodName::core("ping").unwrap(),
        payload: Vec::new(),
    })
}

fn response_ok() -> Message {
    Message::Response(Response {
        id: RequestId(7),
        body: ResponseBody::Ok(Vec::new()),
    })
}

fn request_with_payload() -> Message {
    Message::Request(Request {
        id: RequestId(7),
        method: MethodName::core("ping").unwrap(),
        payload: vec![1, 2, 3],
    })
}

fn response_ok_with_payload() -> Message {
    Message::Response(Response {
        id: RequestId(7),
        body: ResponseBody::Ok(vec![9, 8]),
    })
}

fn response_error() -> Message {
    Message::Response(Response {
        id: RequestId(7),
        body: ResponseBody::Error(WireError::new(ErrorCode::UnknownRequest, "nope")),
    })
}

fn error() -> Message {
    Message::Error(WireError::new(ErrorCode::Protocol, "bad"))
}

fn handshake() -> Handshake {
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

fn assert_vector(message: Message, expected_hex: &str) {
    let expected = hex::decode(expected_hex).expect("vector is valid hex");
    let encoded = ksync::proto::encode(&message).expect("message encodes");
    assert_eq!(encoded, expected, "encoded bytes changed");

    let decoded = ksync::proto::decode(&expected).expect("vector decodes");
    let reencoded = ksync::proto::encode(&decoded).expect("decoded message encodes");
    assert_eq!(reencoded, expected, "decode/encode is not stable");
}

#[test]
fn message_vectors_are_stable() {
    assert_vector(hello(), HELLO);
    assert_vector(hello_ack(), HELLO_ACK);
    assert_vector(auth(), AUTH);
    assert_vector(auth_ok(), AUTH_OK);
    assert_vector(request(), REQUEST);
    assert_vector(request_with_payload(), REQUEST_WITH_PAYLOAD);
    assert_vector(response_ok(), RESPONSE_OK);
    assert_vector(response_ok_with_payload(), RESPONSE_OK_WITH_PAYLOAD);
    assert_vector(response_error(), RESPONSE_ERROR);
    assert_vector(error(), ERROR);
}

#[test]
fn auth_transcript_and_signature_are_stable() {
    let handshake = handshake();

    assert_eq!(
        hex::encode(handshake.transcript(Role::Initiator)),
        TRANSCRIPT_INITIATOR
    );
    assert_eq!(
        hex::encode(handshake.transcript(Role::Acceptor)),
        TRANSCRIPT_ACCEPTOR
    );

    let initiator = Identity::from_seed([1; 32]);
    let signature = handshake.sign(&initiator, Role::Initiator).unwrap();
    assert_eq!(hex::encode(signature), SIGNATURE_INITIATOR);
}
