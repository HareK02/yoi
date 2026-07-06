# worker-runtime

`worker-runtime` owns the Runtime authority surface for Worker management. A Runtime process bundles Worker lifecycle management, the HTTP/WebSocket control API, and the Worker execution backend.

## Run the local Runtime server

From the repository root:

```bash
cargo run -p worker-runtime \
  --features ws-server,fs-store \
  --bin worker-runtime-rest-server \
  -- --workspace .
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
  -- --workspace . --bind 127.0.0.1:38800
```

`--workspace` is currently a legacy bootstrap input for the v0 local materializer / Worker profile resolution path. It is not intended to be the long-term Runtime identity or a single-workspace binding. Future Runtime launches should receive Workspace / Repository context through Worker launch requests and config bundles instead.

The REST server is intended for a trusted Backend/proxy, not direct browser access.
