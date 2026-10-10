# worker-runtime

`worker-runtime` owns the Runtime authority surface for Worker management. A Runtime process bundles Worker lifecycle management, the HTTP/WebSocket control API, and the Worker execution backend.

## Run the local Runtime server

From the repository root:

```bash
cargo run -p worker-runtime \
  --bin yoi-runtime \
  -- --bind 127.0.0.1:38800
```

By default the server listens on:

```text
127.0.0.1:38800
```

To bind another address explicitly:

```bash
cargo run -p worker-runtime \
  --bin yoi-runtime \
  -- --bind 0.0.0.0:38800
```

The REST server is intended for a trusted Backend/proxy, not direct browser access.
Workspace requests authenticate the actual HTTP request with the Workspace's current
signing identity. The Runtime verifies signatures against its explicitly enrolled
Workspace public key and binds the Runtime audience, Workspace/Worker scope,
operation, method, path/query, body digest, expiry, and single-use token ID. A
connection test is an ordinary signed `/v1/ping`; no prior handshake or saved
verification evidence is required or consulted.

Keep `trust-workspace add`, `replace`, and `revoke` as explicit operator actions.
A self-reported public key cannot grant trust. Trust record IDs are administrative
only and must not be copied into Backend registration or request claims. Replay
protection uses Workspace plus the trusted key fingerprint, so re-enrolling the
same key or rotating away and back cannot reset consumed requests. Reaccepting a
key does permit fresh, unconsumed requests signed by that key until their short
expiry; it does not create a separate sender-controlled enrollment namespace.

For authenticated remote Runtime setup, see [`../../docs/development/server-runtime-auth.md`](../../docs/development/server-runtime-auth.md).
