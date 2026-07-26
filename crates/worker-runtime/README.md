# worker-runtime

`worker-runtime` owns the Runtime authority surface for Worker management. A Runtime process bundles Worker lifecycle management, the HTTP/WebSocket control API, and the Worker execution backend.

## Run the local Runtime server

From the repository root:

```bash
cargo run -p worker-runtime \
  --features ws-server,fs-store \
  --bin worker-runtime-rest-server \
  -- --bind 127.0.0.1:38800
```

By default the server listens on:

```text
127.0.0.1:38800
```

To bind another address explicitly:

```bash
cargo run -p worker-runtime \
  --features ws-server,fs-store \
  --bin worker-runtime-rest-server \
  -- --bind 0.0.0.0:38800
```

The REST server is intended for a trusted Backend/proxy, not direct browser access.
For authenticated remote Runtime setup, see [`../../docs/development/server-runtime-auth.md`](../../docs/development/server-runtime-auth.md).
