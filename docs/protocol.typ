#set document(title: "ksync protocol")
#set page(numbering: "1", margin: 2cm)
#set text(size: 10pt)
#set heading(numbering: "1.1")
#set par(justify: true)

#align(center)[
  #text(size: 20pt, weight: "bold")[ksync wire protocol]
  #v(0.3em)
  #text(size: 11pt)[Version 1, draft]
]

#outline()

= Overview

ksync has three processes:

#table(
  columns: (auto, 1fr),
  stroke: 0.5pt,
  [Server], [TLS listener, application identity, method registry, storage (future)],
  [Daemon], [Per-user process that owns identities, server connections, retry, keepalive],
  [CLI], [Local control client; talks to the daemon over a Unix socket],
)

The network protocol is a request/response protocol over TLS. Method payloads
are opaque to the transport. This document describes version 1 of that
protocol, the method layer, the local control protocol, and the configuration
files.

= Transport

- TLS 1.3 only.
- Client and server must negotiate the ALPN protocol `ksync/1`.
- The key-exchange group list is replaced, not reordered, with exactly one
  entry: `X25519MLKEM768`. A peer without ML-KEM fails the handshake; there is
  no classical fallback.
- The server presents a certificate chain. The client verifies it against
  bundled WebPKI roots, plus an optional configured CA file. The operating
  system trust store is not used.
- No TLS client certificates. Client identity is established by the
  application handshake.
- No certificate pinning. The TLS certificate identifies the server name;
  the Ed25519 identity below identifies the ksync peer.
- Post-quantum key exchange covers confidentiality. Server authentication uses
  the certificate's signature algorithm; version 1 application identities use
  Ed25519.

The channel binding is the TLS exporter:

#table(
  columns: (auto, 1fr),
  stroke: 0.5pt,
  [Label], [`EXPORTER-ksync-auth-v1`],
  [Context], [selected protocol version, `u16` big-endian],
  [Length], [32 bytes],
)

= Framing

Every frame is:

```text
+----------------+----------------------------------+
| u32 LE length  | postcard-encoded Message         |
+----------------+----------------------------------+
```

- The length counts the postcard body, not the 4-byte header.
- Length 0 is invalid.
- Maximum frame body: 16 MiB. The check happens before allocation.
- The decoder consumes the entire body. Trailing bytes are an error.
- Additional per-message limits:

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Message], [Limit],
  [`Hello`, `HelloAck`], [4 KiB],
  [`Auth`, `AuthOk`], [8 KiB],
  [`Error` detail], [1 KiB],
  [Other messages], [16 MiB frame limit],
)

= Message schema

Messages are postcard-encoded. The variant index is the wire tag. The schema
is append-only: existing variants are never reordered, removed, or reused.
New fields are carried in new variants.

#table(
  columns: (auto, auto, 1fr),
  stroke: 0.5pt,
  [Index], [Variant], [Direction],
  [0], [`Hello`], [initiator to acceptor],
  [1], [`HelloAck`], [acceptor to initiator],
  [2], [`Auth`], [initiator to acceptor],
  [3], [`AuthOk`], [acceptor to initiator],
  [4], [`Request`], [either direction],
  [5], [`Response`], [either direction],
  [6], [`Error`], [either direction; terminal],
  [7], [reserved], [`Event`, future],
)

Struct fields are encoded in declaration order. Fixed-size byte arrays are
encoded without a length prefix. `Vec<u8>` fields marked `serde_bytes` are
encoded as a length-prefixed byte string.

== Hello

```rust
struct Hello {
    version_min: u16,
    version_max: u16,
    sig_alg: u8,          // 0 = Ed25519
    features: u64,
    initiator_nonce: [u8; 32],
    initiator_id: [u8; 32],
    initiator_pub: [u8; 32],
    acceptor_id: [u8; 32],
}
```

== HelloAck

```rust
struct HelloAck {
    version: u16,
    sig_alg: u8,
    features: u64,
    acceptor_nonce: [u8; 32],
    acceptor_id: [u8; 32],
    acceptor_pub: [u8; 32],
    initiator_id: [u8; 32],
}
```

== Auth and AuthOk

```rust
struct Auth {
    sig_alg: u8,
    signature: byte string,
}

struct AuthOk {
    sig_alg: u8,
    signature: byte string,
}
```

Ed25519 signatures are 64 bytes. The signature field is variable-length so a
future algorithm can be added without a new message variant; the length is
validated against `sig_alg`.

== Request and Response

```rust
struct Request {
    id: u64,
    method: string,
    payload: byte string,
}

struct Response {
    id: u64,
    body: ResponseBody,
}

enum ResponseBody {
    Ok(byte string),   // variant 0
    Error(WireError),  // variant 1
}
```

The response id echoes the request id. Version 1 permits one request in flight
per connection and processes requests sequentially. The initiator is the only
requester in version 1. Id 0 is reserved by convention; current code echoes
whatever id the peer sends.

== Error

```rust
struct WireError {
    code: u16,
    detail: byte string,
}
```

`Message::Error` is terminal: the sender closes the connection after sending
it. `ResponseBody::Error` is a per-request failure and does not close the
connection.

= Handshake

After the TLS handshake and ALPN check:

```text
initiator                              acceptor
    | --------------- Hello -------------> |
    | <------------- HelloAck ------------- |
    | --------------- Auth --------------> |
    | <-------------- AuthOk -------------- |
    | ------------- Request --------------> |
    | <------------ Response --------------- |
```

== Acceptor checks on Hello

1. The client's version range contains a supported version. The selected
   version is the highest mutually supported version; version 1 supports
   exactly version 1.
2. `sig_alg` is supported.
3. `acceptor_id` equals the server's own identity id.
4. `initiator_id` equals the fingerprint of `initiator_pub`.
5. `initiator_id` is present in the server's allowlist.

Failure of steps 1 and 2 produces `Error` with `VersionMismatch` or
`Protocol`. Failures of steps 3 to 5 produce `Error` with `AuthFailed` and an
empty detail.

== HelloAck

The server chooses `features` as the intersection of the client's features
and the server's known features. In version 1 the known set is empty, so the
selected value is 0. The server generates a fresh 32-byte nonce, computes the
exporter, and sends `HelloAck`.

The client checks:

- selected version and `sig_alg`;
- `acceptor_id` equals the expected server id;
- `acceptor_id` matches the fingerprint of `acceptor_pub`;
- echoed `initiator_id` equals its own id;
- selected features are a subset of those it offered.

== Auth and AuthOk

Both sides build the transcript below. The initiator signs it with role
`0x01`, sends the signature in `Auth`. The acceptor verifies it, signs the same
transcript with role `0x02`, and sends the signature in `AuthOk`. The
initiator verifies it.

Ed25519 verification uses strict verification. Signature length must equal 64
bytes. Authentication is repeated for every connection. The negotiated
`sig_alg` governs verification; the duplicate `sig_alg` field in `Auth` and
`AuthOk` is informational in version 1.

= Authentication transcript

The transcript is independent of postcard:

```text
"AUTH-ksync-v1"
u16_be selected_version
u64_be selected_features
u8     sig_alg
u8     role                    // 0x01 initiator, 0x02 acceptor
u32_be length + initiator_nonce
u32_be length + acceptor_nonce
u32_be length + initiator_pub
u32_be length + acceptor_pub
u32_be length + initiator_id
u32_be length + acceptor_id
u32_be length + exporter
```

All length prefixes are big-endian. The role byte is not transmitted; it is
derived from the side producing the signature. It prevents a signature from
being reflected in the other direction. The exporter binds the signature to
the TLS session, so a relay that terminates two TLS sessions cannot reuse the
signature.

Identity ids are:

```text
SHA-256("ksync-id-v1" || sig_alg || public_key)
```

A 32-byte Ed25519 public key therefore has a 32-byte id.

= Methods

== Names

A method name is `namespace:local`.

- Namespace: ASCII lowercase, starts with a letter, then letters, digits, or
  `-`. Maximum 32 bytes. Reserved: `core`, `sys`, `admin`.
- Local name: ASCII lowercase, starts with a letter, then letters, digits,
  `-`, or `_`. Maximum 31 bytes.
- Full name maximum: 64 bytes.
- Exactly one colon. No whitespace, no Unicode, no case folding, no aliases.
- Invalid names are rejected, not rewritten.

Core methods use the `core` namespace. Plugin methods use a namespace assigned
by the host. A plugin registers only the local part; the host prefixes the
namespace.

== Registration

`MethodList` stages methods under their local names. `Registry::register_all`
validates and inserts them atomically under a namespace. A duplicate or
invalid name leaves the registry unchanged. `Registry::register_plugin`
rejects `core`, `sys`, and `admin`; `register_all` is used by the host for
core registration.

== Dispatch

For each `Request`:

1. The request envelope is decoded and the method name validated.
2. The registry looks up the full name. Unknown name: `UnknownRequest`.
3. Payload length is checked against the method's input limit.
4. The payload is decoded with postcard; trailing bytes are rejected.
5. The handler runs.
6. Output length is checked against the method's output limit.
7. The result is sent as `ResponseBody::Ok` or `ResponseBody::Error`.

Default input and output limit: 16 MiB. A method may set a smaller limit at
registration.

== Built-in methods

#table(
  columns: (auto, auto, auto, 1fr),
  stroke: 0.5pt,
  [Name], [Input], [Output], [Meaning],
  [`core:ping`], [`()`], [`()`], [liveness; postcard unit is zero bytes],
)

No method discovery method exists yet. Clients know built-in methods
statically. There is no per-method authorization.

== Method errors

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Method error], [Wire code],
  [`InvalidInput`], [`InvalidRequest`],
  [`NotFound`], [`NotFound`],
  [`Conflict`], [`Conflict`],
  [`TooLarge`], [`TooLarge`],
  [`UnknownMethod`], [`UnknownRequest`],
  [`Internal`], [`Internal`],
)

Detail text is truncated to 1 KiB. Handlers must not place keys, paths, or
internal stack details in the detail.

= Error codes

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Code], [Meaning],
  [0], [`Protocol`],
  [1], [`VersionMismatch`],
  [2], [`AuthFailed`],
  [3], [`Unauthenticated`],
  [4], [`InvalidRequest`],
  [5], [`UnknownRequest`],
  [6], [`NotFound`],
  [7], [`Conflict`],
  [8], [`TooLarge`],
  [9], [`RateLimited`],
  [10], [`Internal`],
  [11], [`Shutdown`],
)

Codes are append-only. A receiver must preserve an unknown numeric code.

= Limits and timeouts

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Item], [Value],
  [Server TLS handshake timeout], [5 s],
  [Client connect and TLS timeout], [10 s each],
  [Application auth timeout], [30 s both sides],
  [Idle connection timeout], [5 min],
  [Graceful shutdown wait], [5 s],
  [Default maximum connections], [128],
  [Daemon keepalive], [90 s],
  [Daemon reconnect backoff], [1 s to 60 s, doubling],
  [Daemon command timeout], [45 s],
  [Request in flight], [1],
)

The server stops accepting on SIGINT or SIGTERM, signals connection tasks,
sends TLS `close_notify` where possible, and waits up to 5 seconds before
aborting remaining tasks.

= Daemon control protocol

The CLI talks to the daemon over a Unix domain socket. It uses the same
framing and postcard codec as the network protocol, but a separate schema and
version.

- Control version: 1.
- One request and one response per connection; the connection is then closed.
- Default socket: `$XDG_RUNTIME_DIR/ksync/control.sock`; fallback
  `/tmp/ksync-<uid>/control.sock`.
- Socket mode `0600`. A directory created by the daemon is `0700`.
- A stale socket is removed only if connecting fails with `NotFound` or
  `ConnectionRefused`.

```rust
struct ControlRequest {
    version: u16,
    command: ControlCommand,
}

enum ControlCommand {
    Status,
    Ping { server: Option<String> },
    Sync { server: Option<String> },
}

struct ControlResponse {
    version: u16,
    result: ControlResult,
}

enum ControlResult {
    Status(DaemonStatus),
    Pong { server: String, rtt_micros: u64 },
    Ok,
    Error { code: u16, detail: String },
}

struct DaemonStatus {
    pid: u32,
    servers: Vec<ServerStatus>,
}

struct ServerStatus {
    name: String,
    connected: bool,
    last_error: Option<String>,
}
```

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Control code], [Meaning],
  [0], [bad version],
  [1], [unknown server],
  [2], [not connected],
  [3], [timeout],
  [4], [not implemented],
  [5], [internal],
)

`Sync` currently returns code 4. The daemon keeps one connection per
configured server, sends `core:ping` every 90 seconds, reconnects with
exponential backoff, and reports connection state through `Status`.

= Configuration

Client and server configuration are separate TOML files. Paths in a TOML file
are relative to that file. Paths supplied on the command line are relative to
the invoking directory.

Identity files contain a raw 32-byte Ed25519 seed. They are created on first
use with mode `0600` and are read again on later starts. A malformed file is an
error; it is never regenerated silently.

Commands:

```text
ksync [--client-config PATH] daemon
ksync [--client-config PATH] status
ksync [--client-config PATH] ping [SERVER] [--direct]
ksync [--client-config PATH] sync [SERVER]
ksync server [--config PATH]
```

== Client

Default path: `$XDG_CONFIG_HOME/ksync/config.toml`, otherwise
`$HOME/.config/ksync/config.toml`. Override with `--client-config` or
`KSYNC_CLIENT_CONFIG`.

```toml
[client]
identity = "identity.key"                 # optional
control_socket = "state/control.sock"     # optional

[[servers]]
name = "home"
addr = "sync.example.com:8443"
server_name = "sync.example.com"
server_id = "0123...64 hex characters"
ca = "home-ca.pem"                        # optional
```

#table(
  columns: (auto, auto, 1fr),
  stroke: 0.5pt,
  [Field], [Default], [Meaning],
  [`client.identity`], [`identity.key` beside the config], [Ed25519 seed path],
  [`client.control_socket`], [XDG runtime path], [daemon control socket],
  [`servers[].name`], [required], [unique profile name],
  [`servers[].addr`], [required], [TCP address],
  [`servers[].server_name`], [required], [TLS SNI and certificate name],
  [`servers[].server_id`], [required], [server identity id, 64 hex characters],
  [`servers[].ca`], [none], [extra trust anchor; bundled WebPKI roots always apply],
)

At least one server is required. Names must be unique. The optional CA file is
a trust anchor, not a certificate pin.

== Server

Default: no config file. Pass `--config` or set `KSYNC_SERVER_CONFIG`. Without
a file, flags and built-in defaults apply.

```toml
[server]
addr = "127.0.0.1:8443"
data_dir = "data"
identity_key = "identity.key"
tls_cert = "tls/server.crt"
tls_key = "tls/server.key"
authorized_clients = "authorized_clients.txt"
max_conn = 128
dev = false
```

#table(
  columns: (auto, auto, 1fr),
  stroke: 0.5pt,
  [Field], [Default], [Meaning],
  [`addr`], [`127.0.0.1:8443`], [listen address; port 0 selects an ephemeral port],
  [`data_dir`], [`data`], [runtime state directory],
  [`identity_key`], [`<data_dir>/identity.key`], [Ed25519 seed path],
  [`tls_cert`], [none], [PEM certificate chain],
  [`tls_key`], [none], [PEM private key],
  [`authorized_clients`], [required], [allowlist file],
  [`max_conn`], [`128`], [maximum concurrent connections, at least 1],
  [`dev`], [`false`], [generate a local CA and `localhost` leaf],
)

Precedence: command line and environment override TOML, which overrides
defaults.

`tls_cert` and `tls_key` are required together unless `dev = true`.
`dev = true` cannot be combined with either TLS field. `authorized_clients`
is always required.

The allowlist file contains one 64-character hex identity id per line.
`#` starts a comment. Blank lines are ignored.

Server environment variables:

#table(
  columns: (auto, auto),
  stroke: 0.5pt,
  [Variable], [Field],
  [`KSYNC_SERVER_CONFIG`], [config path],
  [`KSYNC_ADDR`], [`addr`],
  [`KSYNC_DATA`], [`data_dir`],
  [`KSYNC_SERVER_IDENTITY_KEY`], [`identity_key`],
  [`KSYNC_TLS_CERT`], [`tls_cert`],
  [`KSYNC_TLS_KEY`], [`tls_key`],
  [`KSYNC_AUTHORIZED_CLIENTS`], [`authorized_clients`],
)

== Development

`examples/config/` contains working client and server configs with public,
committed Ed25519 seeds. They are for repository-local development only. The
server uses port `18443` and writes runtime state under
`examples/config/state/`, which is gitignored. `. ./env.sh` sets
`KSYNC_CLIENT_CONFIG` and `KSYNC_SERVER_CONFIG` for the current shell.

= Compatibility and tests

- `PROTOCOL_VERSION` is 1. The client offers exactly `1..=1`; the server
  supports exactly version 1. `Hello`/`HelloAck` carry ranges so later
  versions can negotiate a common value.
- Message variant indices, response-body indices, signature-algorithm values,
  and error codes are wire contracts. They are append-only.
- Method names are exact byte strings. No aliases or normalization.
- `tests/golden.rs` fixes the bytes of every message variant and the
  authentication transcript and signature for fixed inputs. A schema change
  must update these vectors deliberately.
- `tests/mitm.rs` runs an application-level relay that terminates two TLS
  sessions. Authentication must fail because the exporter differs.
- The daemon integration tests cover control requests, clean shutdown, socket
  removal, reconnect after server restart, and duplicate-daemon rejection.
