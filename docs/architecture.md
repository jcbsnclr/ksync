# Architecture

This document describes the code that exists on the `new-sync` branch today.
The storage and sync design in [ROADMAP.md](../ROADMAP.md) is shown only in the
final [implemented versus planned](#10-implemented-versus-planned) section and
is clearly marked as future work.

The flow and state diagrams are Mermaid, so GitHub renders them directly from
this file. The wire frame is shown as an ASCII code block.

## Contents

1. [Processes and trust boundaries](#1-processes-and-trust-boundaries)
2. [Code map](#2-code-map)
3. [Handshake and authentication](#3-handshake-and-authentication)
4. [Server runtime](#4-server-runtime)
5. [Daemon runtime](#5-daemon-runtime)
6. [Wire format](#6-wire-format)
7. [Method layer](#7-method-layer)
8. [Configuration and identity](#8-configuration-and-identity)
9. [Tests](#9-tests)
10. [Implemented versus planned](#10-implemented-versus-planned)
11. [Limits and timeouts](#11-limits-and-timeouts)

## 1. Processes and trust boundaries

```mermaid
flowchart LR
    subgraph user["User machine"]
        CLI["ksync CLI<br/>status, ping, sync"]
        SOCK["Unix control socket<br/>0600"]
        CTRL["control listener"]
        DAEMON["daemon<br/>profile status and routing"]
        MAT["client config.toml<br/>plus identity.key"]
        subgraph tasks["one task per server profile"]
            TASK["connect, keepalive, backoff"]
        end
    end

    subgraph host["Server host"]
        SRV["ksync server"]
        SID["server identity.key<br/>Ed25519 seed, 0600"]
        ALLOW["authorized_clients.txt"]
        REG["method registry<br/>core:ping"]
        TLS["TLS identity<br/>PEM cert and key<br/>or generated dev CA"]
    end

    CLI -->|"ControlRequest / ControlResponse"| SOCK
    SOCK --> CTRL
    CTRL --> DAEMON
    DAEMON --> TASK
    MAT -.->|"loaded once by the daemon"| DAEMON
    TASK -->|"TCP and TLS 1.3<br/>ALPN ksync/1"| SRV
    SRV --- SID
    SRV --- ALLOW
    SRV --- REG
    SRV --- TLS
    CLI -.->|"ping --direct"| SRV
```

There are three roles:

| Role | Responsibility |
|---|---|
| **CLI** | Parses commands, loads the client config, and either talks to the daemon over the control socket or, for `ping --direct`, runs the client handshake itself. It never owns a long-lived network connection otherwise. |
| **Daemon** | One process per user. Owns the client identity, binds the control socket, runs one connection task per configured server, and maintains connection status. |
| **Server** | Listens on TCP, terminates TLS 1.3, authenticates clients by Ed25519 identity and allowlist, and dispatches authenticated method calls. |

Trust is split into two independent planes:

- **TLS** proves _which server host_ was reached and protects the channel.
- **Ed25519 application auth** proves _which user/device_ is on the other end.

Neither depends on the other's certificate being pinned. The TLS exporter binds
the two planes together for the lifetime of the connection.

## 2. Code map

```mermaid
flowchart LR
    subgraph entry["Entry"]
        MAIN["main.rs<br/>CLI and command routing"]
    end

    subgraph runtime["Process runtime"]
        SERVER["server.rs<br/>TLS listener, auth, requests"]
        DAEMON["daemon.rs<br/>control socket, per-server tasks"]
        CLIENT["client.rs<br/>TLS and application handshake"]
    end

    subgraph application["Application"]
        METHODS["methods.rs<br/>built-in core methods"]
        METHOD["method.rs<br/>typed dispatch and registry"]
    end

    subgraph security["Security"]
        AUTH["auth.rs<br/>transcript and exporter"]
        TLS["tls.rs<br/>TLS configuration"]
        IDENT["identity.rs<br/>Ed25519 ids and allowlist"]
    end

    subgraph wire["Wire"]
        PROTO["proto/mod.rs<br/>messages, names, limits"]
        FRAME["proto/frame.rs<br/>u32 LE framing"]
    end

    subgraph support["Support"]
        CONFIG["config.rs<br/>TOML resolution"]
        CONTROL["control.rs<br/>local control schema"]
        SHUTDOWN["shutdown.rs<br/>signal fan-out"]
    end

    MAIN -->|"commands"| runtime
    MAIN -->|"config and control"| support
    runtime -->|"built-in methods"| application
    runtime -->|"auth and TLS"| security
    runtime -->|"config, control, shutdown"| support
    application -->|"encode and decode"| wire
    security -->|"ids, ALPN, messages"| wire
    support -->|"control schema, ids"| wire
    PROTO --> FRAME
```

The layer map shows the primary dependency direction. The table below records
each module's actual imports.

`lib.rs` exposes these modules as a library so the integration tests can drive
the real listener and daemon in-process.

| Module | Responsibility | Key dependencies |
|---|---|---|
| `main.rs` | clap command parsing, tracing setup, command routing | `server`, `daemon`, `client`, `control`, `config`, `methods`, `proto` |
| `lib.rs` | Library exports for tests | all modules |
| `proto/mod.rs` | Message schema, method names, error codes, features, limits, postcard codec, version negotiation | `proto::frame` |
| `proto/frame.rs` | `u32` LE length-prefixed frames, bounded allocation, 16 MiB maximum | none |
| `tls.rs` | rustls provider, TLS 1.3-only configs, PQ-only key exchange, development CA and leaf | `proto` |
| `identity.rs` | Ed25519 seeds, identity ids, signature helpers, allowlist parsing, `0600` key files | `proto` |
| `auth.rs` | Authentication transcript, TLS exporter, role-bound sign and verify | `identity`, `proto` |
| `method.rs` | `Method` and erased dispatch, per-method limits, `MethodList`, atomic `Registry` | `proto` |
| `methods.rs` | Built-in core methods and the core registry | `method`, `proto` |
| `server.rs` | Listener, connection limit, per-connection handshake and request loop | `auth`, `config`, `identity`, `method`, `methods`, `proto`, `shutdown`, `tls` |
| `client.rs` | TCP/TLS connect, application handshake, typed request helper | `auth`, `config`, `identity`, `method`, `proto`, `tls` |
| `daemon.rs` | Control listener, per-server tasks, status, keepalive, reconnect | `client`, `config`, `control`, `identity`, `methods`, `proto`, `shutdown` |
| `control.rs` | Local control schema, framing, request helper, stale socket handling | `proto` |
| `config.rs` | Client and server TOML discovery, path resolution, CLI overrides | `identity`, `proto` |
| `shutdown.rs` | `watch`-based shutdown signal, SIGINT/SIGTERM installation | none |

## 3. Handshake and authentication

### 3.1 Connection sequence

```mermaid
sequenceDiagram
    autonumber
    participant C as client
    participant S as server

    Note over C,S: TCP connect
    C->>S: TLS 1.3 ClientHello, ALPN ksync/1, X25519MLKEM768
    S-->>C: ServerHello and certificate
    Note over C: Verify against bundled WebPKI roots<br/>plus an optional configured CA file.<br/>No client certificate is requested.
    C->>S: Hello
    Note over S: Check version range, sig_alg,<br/>expected server id, client key/id,<br/>and client allowlist.

    alt handshake rejected
        S-->>C: Error VersionMismatch or AuthFailed
        Note over C,S: The connection closes.
    else handshake accepted
        S-->>C: HelloAck
        Note over C: Verify version, sig_alg, server id,<br/>echoed client id, and feature subset.
        C->>S: Auth, Ed25519 signature
        Note over S: Rebuild the transcript with the<br/>TLS exporter and verify the signature.
        S-->>C: AuthOk, Ed25519 signature
        Note over C: Verify the acceptor signature.
        C->>S: Request id, method, payload
        S-->>C: Response Ok or Error
    end
```

The server sends `HelloAck` only after it has accepted the client's version,
signature algorithm, server id, key/id pair, and allowlist entry. Failures in
the last three cases deliberately produce the same `AuthFailed` with an empty
detail, so an unauthenticated peer cannot enumerate identities.

`Hello` carries a version range and `HelloAck` selects one version. The current
build supports exactly version `1`. Features are negotiated by intersection.
The client currently offers `Features::NONE` and the server's known set is
empty, so the selected feature mask is always `0`.

### 3.2 What each authentication layer proves

```mermaid
flowchart LR
    C["client"]
    S["server"]

    subgraph tls["TLS plane: which host"]
        ROOTS["Bundled WebPKI roots<br/>plus optional CA file"]
        CERT["Server certificate chain"]
        ROOTS -->|"verifies"| CERT
        CERT -->|"belongs to"| S
        C -->|"TLS 1.3, PQ-only group<br/>ALPN ksync/1"| S
    end

    subgraph app["Application plane: which user/device"]
        CK["Client Ed25519 key"]
        SK["Server Ed25519 key"]
        T["Signed transcript<br/>version, features, sig_alg, role<br/>both nonces, both public keys<br/>both ids, TLS exporter"]
        V["Mutual verification"]
        AL["Client id must be in the<br/>server allowlist"]
        CK -->|"signs"| T
        SK -->|"signs"| T
        T --> V
        V --> AL
        AL --> SK
    end

    S -->|"TLS exporter is mixed into the transcript"| T
```

The exported channel binding is:

| Field | Value |
|---|---|
| Label | `EXPORTER-ksync-auth-v1` |
| Context | Selected protocol version as `u16` big-endian |
| Length | 32 bytes |

Application identity ids are derived, not configured:

```text
id = SHA-256("ksync-id-v1" || sig_alg || public_key)
```

The transcript is independent of postcard and contains, in order:
`"AUTH-ksync-v1"`, selected version, feature mask, signature algorithm, role
byte (`0x01` initiator, `0x02` acceptor), length-prefixed initiator nonce,
acceptor nonce, initiator public key, acceptor public key, initiator id,
acceptor id, and exporter. Each side signs with its own role. A signature
cannot be reflected in the other direction because the role byte is signed.

### 3.3 Why a relay cannot reuse a signature

```mermaid
sequenceDiagram
    autonumber
    participant C as client
    participant R as relay
    participant S as real server

    C->>R: TLS session A
    R->>S: TLS session B
    C->>R: Hello
    R->>S: Hello
    S-->>R: HelloAck
    R-->>C: HelloAck
    Note over C: Sign transcript with exporter A
    C->>R: Auth, signature over exporter A
    R->>S: Auth, same signature
    Note over S: Rebuild transcript with exporter B.<br/>The exporter bytes differ, so verification fails.
    S-->>R: Error AuthFailed
    R-->>C: Error AuthFailed
```

The relay test in `tests/mitm.rs` terminates two separate TLS sessions and
forwards application frames verbatim. The signatures are valid in isolation,
but the exporter differs between sessions, so the server rejects the relayed
`Auth`.

## 4. Server runtime

### 4.1 Accept loop and connection limiting

```mermaid
flowchart TD
    A["serve loop"] --> B{"shutdown signalled?"}
    B -- yes --> Z["Drop listener<br/>drain JoinSet for up to 5 s<br/>abort remaining tasks"]
    B -- no --> C["Acquire semaphore permit<br/>at most max_conn live tasks"]
    C --> D["Accept TCP connection"]
    D -- "accept error" --> E["Log warning and continue"]
    E --> A
    D -- "connection" --> F["Spawn connection task<br/>permit moves into the task"]
    F --> G["TLS handshake, auth, request loop"]
    G --> H["Task ends"]
    H --> I["Permit is released by Drop"]
    I --> A
```

The permit is acquired **before** `accept`. At capacity the process stops
pulling connections from the kernel queue, so the backlog absorbs the burst
instead of the process holding sockets it cannot serve. `max_conn` defaults to
128 and is clamped to at least one at startup. A failed `accept` is logged and
does not bring down the listener.

Shutdown is a single `watch` event. The server stops accepting, gives live
connections five seconds to finish, aborts whatever remains, and waits up to
one more second for the aborted tasks.

### 4.2 Connection state machine

```mermaid
stateDiagram-v2
    state "TLS handshake" as TlsHandshake
    state "ALPN check" as AlpnCheck
    state "Application auth" as AppAuth
    state "Request loop" as Ready
    state "Closing" as Closing

    [*] --> TlsHandshake
    TlsHandshake --> AlpnCheck: TLS 1.3 complete
    TlsHandshake --> Closing: 5 s timeout or TLS error
    AlpnCheck --> AppAuth: ALPN is ksync/1
    AlpnCheck --> Closing: ALPN mismatch
    AppAuth --> Ready: AuthOk sent
    AppAuth --> Closing: 30 s timeout or auth failure
    Ready --> Ready: Request then Response
    Ready --> Closing: peer Error, EOF, or protocol violation
    Ready --> Closing: idle for 5 minutes
    Ready --> Closing: shutdown signal
    Closing --> [*]: best-effort close_notify
```

The TLS handshake must complete within five seconds. The whole application
handshake from `Hello` through `AuthOk` gets thirty seconds. An authenticated
connection may sit idle for five minutes. Before closing normally the server
makes a best-effort TLS `close_notify`; a clean peer EOF is treated as ordinary
disconnect, not as an error.

### 4.3 Request loop

The loop is deliberately sequential:

1. Read one framed message with a 16 MiB limit.
2. If it is a `Request`, dispatch it through the registry and write one
   `Response`.
3. If it is a terminal `Error`, log and close.
4. Any other message is a protocol violation: send `Error(Protocol)` and close.
5. Clean EOF, idle timeout, or shutdown ends the loop.

Version 1 permits one request in flight per connection. There is no
multiplexing, server push, or interleaved stream. `request.id` is echoed by the
server, and each request is logged with its method and elapsed time.

## 5. Daemon runtime

### 5.1 Components

```mermaid
flowchart LR
    CLI["ksync CLI"] -->|"ControlRequest"| LISTENER["UnixListener<br/>control.sock, 0600"]
    LISTENER --> HANDLER["handle_control<br/>one task per connection"]
    HANDLER -->|"Status"| STATUS["shared status<br/>per server profile"]
    HANDLER -->|"Ping"| QUEUE["mpsc queue<br/>capacity 8"]
    HANDLER -->|"Sync"| NI["Error NOT_IMPLEMENTED"]
    QUEUE --> TASK["server_task<br/>one per profile"]
    TASK -->|"client::connect"| SRV["ksync server"]
    TASK -->|"core:ping every 90 s"| SRV
    TASK -->|"connected, last_error"| STATUS
    HANDLER -->|"ControlResponse"| CLI
```

The daemon loads the client identity once and shares it across server tasks.
Each task owns its TLS connection and processes commands one at a time. The
control plane never sees identities or TLS settings; it only names a configured
server profile.

The control socket is `0600`. A directory created by the daemon is `0700`; an
existing shared directory is never chmodded. On startup, a stale socket is
removed only when connecting to it fails with `NotFound` or
`ConnectionRefused`. If a live daemon answers, startup fails instead of
stealing the socket.

### 5.2 Per-server task state machine

```mermaid
stateDiagram-v2
    [*] --> Connecting
    Connecting --> Connected: TCP, TLS, and Ed25519 auth succeed
    Connecting --> Backoff: connect, TLS, or auth error
    Connected --> Connected: control Ping or keepalive tick
    Connected --> Backoff: request failure or connection loss
    Connected --> [*]: shutdown or command channel closed
    Backoff --> Connecting: backoff deadline elapses
    Backoff --> Backoff: answer Ping with not connected
    Backoff --> [*]: shutdown or command channel closed

    note right of Connecting
        status.connected = false
        status.last_error records the failure
    end note
    note right of Connected
        keepalive is core:ping every 90 s
        success resets backoff to 1 s
    end note
    note right of Backoff
        backoff starts at 1 s and doubles to 60 s
        control requests are still answered
    end note
```

While connected, the task selects between three events: shutdown, a queued
control command, and the keepalive tick. A `Ping` is a real `core:ping` request
over the existing TLS connection; its round-trip time is returned to the CLI.
A request failure marks the profile disconnected and drops into backoff. A
keepalive failure does the same.

While backing off, the task continues to receive control commands so a CLI does
not hang until its own timeout. A ping gets an immediate `not connected` error.
On the next successful connection, the backoff resets to one second.

### 5.3 Control protocol

The control protocol uses the same framing and postcard codec as the network
protocol, but a separate schema and version. One request and one response are
exchanged per Unix socket connection.

| Command | Result | Current behavior |
|---|---|---|
| `Status` | `DaemonStatus { pid, servers }` | Returns the daemon pid and each profile's `connected` and `last_error` |
| `Ping { server }` | `Pong { server, rtt_micros }` | Sends `core:ping` through the selected server task |
| `Sync { server }` | `Error` | Always returns control code `NOT_IMPLEMENTED` (4) |

| Control code | Meaning |
|---|---|
| `0` | Bad version |
| `1` | Unknown server |
| `2` | Not connected |
| `3` | Timeout |
| `4` | Not implemented |
| `5` | Internal |

When several profiles are configured, a ping without a server name is
rejected rather than guessed. `Status` needs no profile; `Sync` currently
returns `NOT_IMPLEMENTED` before it selects one.

### 5.4 CLI routing

```mermaid
flowchart TD
    CLI["ksync"] --> S["server"]
    CLI --> D["daemon"]
    CLI --> ST["status"]
    CLI --> P["ping [server] [--direct]"]
    CLI --> SY["sync [server]"]

    S --> SR["server::run<br/>foreground, SIGINT/SIGTERM"]
    D --> DR["daemon::run<br/>foreground, SIGINT/SIGTERM"]
    ST --> CS["control Status"]
    P --> PD{"--direct?"}
    PD -- yes --> PC["client::connect<br/>TLS plus application auth"]
    PC --> PP["call_core Ping"]
    PD -- no --> CP["control Ping"]
    SY --> CC["control Sync"]
    CS --> SOCK["Unix control socket"]
    CP --> SOCK
    CC --> SOCK
    SOCK --> DM["ksync daemon"]
    DM -->|"Status, Ping"| REPLY["ControlResponse"]
    DM -->|"Sync"| NI["Error NOT_IMPLEMENTED"]
```

The CLI config path is global: `--client-config`, then
`KSYNC_CLIENT_CONFIG`, then the XDG default. `server` and `daemon` run in the
foreground and rely on the shared signal handler for graceful shutdown.

## 6. Wire format

Every frame is a length prefix followed by exactly that many bytes of postcard:

```text
+-----------------------------+------------------------------------------+
| u32 LE body length (4 B)    | postcard-encoded Message (body length B) |
+-----------------------------+------------------------------------------+
```

Rules enforced by `src/proto/frame.rs` and `src/proto/mod.rs`:

- The length counts the postcard body only, not the four-byte prefix.
- A zero-length frame is invalid.
- The body limit is 16 MiB, checked before allocation.
- The decoder must consume the whole body; trailing bytes are an error.
- Framing is owned by ksync, not by postcard, so it can change independently
  of the schema.
- No `usize` is written to the wire. Schema integers are fixed-width.

The message schema is append-only. The postcard variant index is the wire tag:

| Index | Variant | Direction |
|---|---|---|
| `0` | `Hello` | initiator to acceptor |
| `1` | `HelloAck` | acceptor to initiator |
| `2` | `Auth` | initiator to acceptor |
| `3` | `AuthOk` | acceptor to initiator |
| `4` | `Request` | client to server in version 1 |
| `5` | `Response` | server to client in version 1 |
| `6` | `Error` | either direction, terminal |
| `7` | reserved | future `Event`; not used |

Message-specific limits are lower than the frame limit where appropriate:

| Message | Limit |
|---|---|
| `Hello`, `HelloAck` | 4 KiB |
| `Auth`, `AuthOk` | 8 KiB |
| `Error` detail | 1 KiB |
| Other messages | 16 MiB frame limit |

`Message::Error` is terminal and closes the connection. `ResponseBody::Error`
is a per-request failure and does not close it. Error codes are append-only
numeric values (`0` through `11` currently). An unknown code is preserved as
`ErrorCode::Unknown`.

## 7. Method layer

### 7.1 Dispatch pipeline

```mermaid
flowchart TD
    A["Request id, method, payload"] --> B["Read frame, maximum 16 MiB"]
    B --> C["Postcard decode Message<br/>reject trailing bytes"]
    C --> D{"Method name valid?"}
    D -- no --> E["Connection-level decode error"]
    D -- yes --> F{"Registered in namespace?"}
    F -- no --> G["MethodError::UnknownMethod"]
    F -- yes --> H{"payload within max_input?"}
    H -- no --> I["MethodError::TooLarge"]
    H -- yes --> J["ErasedMethod::call_bytes"]
    J --> K{"Postcard input decodes?"}
    K -- no --> L["MethodError::InvalidInput"]
    K -- yes --> M["M::call(ctx, input)"]
    M --> N{"Handler succeeded?"}
    N -- no --> O["Mapped MethodError"]
    N -- yes --> P{"output within max_output?"}
    P -- no --> Q["MethodError::Internal"]
    P -- yes --> R["ResponseBody::Ok"]
    G --> S["ResponseBody::Error WireError"]
    I --> S
    L --> S
    O --> S
    Q --> S
    S --> T["Response sent"]
    R --> T
```

A malformed message envelope or invalid method name fails during `Message`
decoding and is a connection-level error. Failures after a valid request
envelope is dispatched are reported per request, so the connection can
continue.

`Context` currently carries only the authenticated `peer` identity and the
`request_id`. The planned `Store` handle is not present yet.

### 7.2 Method errors

| `MethodError` | Wire code |
|---|---|
| `InvalidInput` | `InvalidRequest` |
| `NotFound` | `NotFound` |
| `Conflict` | `Conflict` |
| `TooLarge` | `TooLarge` |
| `UnknownMethod` | `UnknownRequest` |
| `Internal` | `Internal` |

Detail text is truncated to 1 KiB. Handlers must not put keys, filesystem
paths, or internal stack details into it. In the current build the only
built-in method is `core:ping`, which takes postcard unit input and output
(both zero bytes).

### 7.3 Registration

`MethodList` stages handlers by local name. `Registry::register_all` validates
every name and inserts the whole namespace atomically: an invalid or duplicate
name leaves the registry unchanged. `Registry::register_plugin` rejects the
reserved namespaces `core`, `sys`, and `admin`. There is no plugin loader in
this tree; the registration API exists, but only the core registry is built at
startup.

## 8. Configuration and identity

### 8.1 Client and server resolution

```mermaid
flowchart TD
    A["ksync CLI"] --> B{"--client-config or<br/>KSYNC_CLIENT_CONFIG?"}
    B -- yes --> C["Explicit path"]
    B -- no --> D["XDG_CONFIG_HOME/ksync/config.toml<br/>or HOME/.config/ksync/config.toml"]
    C --> E["ClientConfig::load"]
    D --> E
    E --> F["identity.key<br/>default beside the config"]
    E --> G["control.sock<br/>XDG runtime, or /tmp/ksync-uid"]
    E --> H["server profiles<br/>addr, server_name, server_id, CA"]

    I["ksync server"] --> J{"--config or<br/>KSYNC_SERVER_CONFIG?"}
    J -- yes --> K["Parse [server] TOML"]
    J -- no --> L["No file; use defaults"]
    K --> M["Apply CLI and environment overrides"]
    L --> M
    M --> N["ServerSettings::resolve"]
    N --> O{"dev, or cert plus key?"}
    O -- no --> P["Startup error"]
    O -- yes --> Q{"authorized_clients set<br/>and max_conn at least 1?"}
    Q -- no --> P
    Q -- yes --> R["Bind and serve"]
```

Client paths in TOML resolve relative to the config file, with the sole
exception that an omitted control socket uses the XDG runtime default. Server
CLI paths resolve relative to the invoking directory; server TOML paths
resolve relative to the TOML file. Precedence for the server is CLI and
environment over TOML over defaults.

Key defaults:

| Setting | Default |
|---|---|
| Server listen address | `127.0.0.1:8443` |
| Server data directory | `data` |
| Server identity key | `<data_dir>/identity.key` |
| Server maximum connections | `128` |
| Server client allowlist | Required; no default |
| Server TLS | Required `tls_cert` and `tls_key`, unless `dev = true` |
| Client identity | `identity.key` beside the client config |
| Client control socket | `$XDG_RUNTIME_DIR/ksync/control.sock`, else `/tmp/ksync-<uid>/control.sock` |
| Client servers | At least one; names must be unique |

`dev = true` generates a local CA and a `localhost` leaf under
`<data_dir>/tls/`. It cannot be combined with explicit TLS material. The CA is
a trust anchor for the client, not a certificate pin, so the leaf can rotate.
Half-written dev TLS material is rejected rather than silently regenerated.

### 8.2 Identity material

```mermaid
flowchart LR
    SEED["Raw 32-byte Ed25519 seed<br/>created on first use"] --> KEY["SigningKey"]
    KEY --> PUB["Public key"]
    PUB --> ID["id = SHA-256 of<br/>ksync-id-v1, sig_alg, public key"]
    KEY --> SIGN["Sign handshake transcript"]
    PUB --> VERIFY["Verify peer transcript"]
    ID --> ALLOW["Server allowlist match"]
    ID --> EXPECT["Client expected server_id match"]
```

Identity files contain a raw 32-byte seed, are written with mode `0600`, and
are never logged. Reading tightens an existing file's permissions. A malformed
file is an error; it is not replaced. The checked-in development seeds under
`examples/config/` are public on purpose and must not be used outside local
development.

## 9. Tests

```mermaid
flowchart TD
    T["cargo test"] --> U["Unit tests inside src modules"]
    T --> INT["Integration tests in tests/"]

    U --> P["proto and frame<br/>codec, frozen variants, names, frame limits"]
    U --> AU["auth and identity<br/>direction, reflection, exporter, transcript, ids"]
    U --> M["method registry<br/>dispatch, limits, atomic registration"]
    U --> CF["config and tls<br/>resolution, overrides, PQ-only, key modes"]
    U --> CT["control schema"]

    INT --> G["golden.rs<br/>exact bytes for every message,<br/>both transcripts, one signature"]
    INT --> H["handshake.rs<br/>PQ and TLS 1.3, trust, hostname,<br/>classical rejection, allowlist, ping"]
    INT --> MI["mitm.rs<br/>relay terminating two TLS sessions<br/>must fail authentication"]
    INT --> DA["daemon.rs<br/>lifecycle, reconnect, duplicate daemon,<br/>socket removal on shutdown"]
```

The integration tests use the real `server::serve` loop and real rustls
connections. TLS verification is never disabled: tests add the development CA
as a trust anchor. A classical-only client is expected to fail rather than
downgrade. The daemon tests drive `daemon::run_until` with a test shutdown
channel, so they exercise the real control loop without signals.

## 10. Implemented versus planned

The following diagram is **not current behavior**. It maps the design in
[ROADMAP.md](../ROADMAP.md) onto the current foundation so the intended shape is
visible.

```mermaid
flowchart LR
    subgraph current["In this tree"]
        CLI["CLI"]
        DAEMON["daemon<br/>control, keepalive, reconnect"]
        SERVER["server<br/>TLS, auth, request loop"]
        REG["registry<br/>core:ping"]
        CLI --> DAEMON
        DAEMON -->|"TLS"| SERVER
        SERVER --> REG
    end

    subgraph planned["Designed, not implemented"]
        STORE["Store trait<br/>and MemoryStore"]
        REDB[("redb metadata<br/>plus segment files")]
        CHUNK["chunk has, put, get"]
        ROOTS["manifests, roots,<br/>history, changes"]
        IDX[("client sync index<br/>desired, observed, base,<br/>pending, cursor")]
        FS["filesystem scan<br/>and apply"]
        GC["offline GC<br/>and repack"]
    end

    SERVER -.->|"S0"| STORE
    STORE -.->|"S4"| REDB
    STORE -.->|"S2"| CHUNK
    CHUNK -.->|"S3"| ROOTS
    DAEMON -.->|"S5a"| IDX
    IDX -.-> FS
    ROOTS -.-> IDX
    REDB -.->|"S6"| GC
```

| Area | Current tree | Planned |
|---|---|---|
| Store boundary | None | `Store` trait plus `MemoryStore` |
| Chunk transfer | None | `core:chunk-has`, `core:chunk-put`, `core:chunk-get` with streaming |
| Wire streaming | None | `StreamData`, `StreamEnd`, `StreamAbort`; `Features::STREAMING` |
| Manifests | None | File and directory manifests, deterministic encoding |
| Roots and history | None | Root CAS, revisions, HLC, tombstones, rollback |
| Server storage | None | redb metadata plus append-only segment files |
| Client sync index | None | redb desired/observed/base/pending/apply journal/cursor |
| Filesystem sync | None | Scan, three-way merge, apply journal |
| Records roots | None | Generic content-addressed records |
| GC | None | Offline mark/sweep, repack, compaction |

## 11. Limits and timeouts

| Item | Value |
|---|---|
| Maximum TLS frame body | 16 MiB |
| `Hello` / `HelloAck` message limit | 4 KiB |
| `Auth` / `AuthOk` message limit | 8 KiB |
| `Error` detail limit | 1 KiB |
| Default method input and output limit | 16 MiB |
| Server TLS handshake timeout | 5 s |
| Client TCP connect timeout | 10 s |
| Client TLS handshake timeout | 10 s |
| Application auth timeout | 30 s both sides |
| Authenticated idle timeout | 5 min |
| Default maximum connections | 128, at least 1 |
| Graceful shutdown drain | 5 s, then abort and wait 1 s |
| Daemon keepalive interval | 90 s |
| Daemon reconnect backoff | 1 s doubling to 60 s |
| Daemon control command timeout | 45 s |
| Requests in flight per connection | 1 |
| Control protocol version | 1 |

For the full wire contract, see [protocol.typ](protocol.typ).
