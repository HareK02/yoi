# Runtime HTTP-only production feature dependencies

While validating the Workdir cleanup fix, `cargo check -p worker-runtime --no-default-features --features http-server` failed on dependencies outside the cleanup diff:

- `http_server.rs` and `http_server/runtime_management_api.rs` use retention APIs gated by `fs-store`.
- `workspace_request.rs` imports `futures::StreamExt`, and an HTTP-enabled path in `worker_backend.rs` uses `futures::executor::block_on`, but `dep:futures` is activated by `ws-server`, not `http-server`.

The HTTP-only **test** build with `fs-store,http-server` succeeds because `futures` is also a dev-dependency; it does not prove that configuration's production compile closure. Root `cargo check` and the default Runtime build cover the normal `ws-server,fs-store` configuration.

This dependency mismatch was not repaired as part of the cleanup concurrency fix. Decide the supported feature surface explicitly, then align dependency ownership and retention route gating with it. Keep a production `cargo check` for each supported non-default configuration so dev-dependencies cannot mask this boundary.
