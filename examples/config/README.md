# Development configs

These configs are for running ksync from this repository. They are not a
template for deployment.

**The identity seeds in this directory are public and committed on purpose.**
Anyone with the repository can connect to the development server. The CA and
leaf certificates are generated under `state/` on first run and are not
committed.

## Run

From the repository root:

```sh
. ./env.sh   # optional: sets KSYNC_CLIENT_CONFIG and KSYNC_SERVER_CONFIG

# Terminal 1: server
cargo run -- server
# or: cargo run -- server --config examples/config/server.toml

# Terminal 2: one-shot client
cargo run -- ping --direct

# Or the daemon
cargo run -- daemon
cargo run -- status
cargo run -- ping
```

Without `env.sh`, pass `--config examples/config/server.toml` and
`--client-config examples/config/client.toml` explicitly.

The server listens on `127.0.0.1:18443`, not the usual `8443`, so it does not
collide with a normal instance.

The daemon uses `examples/config/state/control.sock`, so it is isolated from a
real user daemon.

## Reset

```sh
rm -rf examples/config/state
```

The server identity and authorized client stay the same because those seeds are
committed. Only the CA, leaf certificate, and runtime state are regenerated.

## Files

| File | Purpose |
|---|---|
| `server.toml` | server address, dev TLS, allowlist |
| `client.toml` | client identity, server profile, control socket |
| `authorized_clients.txt` | the committed dev client's identity id |
| `dev-server-identity.key` | public Ed25519 seed for the dev server |
| `dev-client-identity.key` | public Ed25519 seed for the dev client |
| `state/` | generated at runtime; gitignored |
