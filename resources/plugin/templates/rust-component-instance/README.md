# Rust Service Plugin Template

This template targets the Component Model-only runtime (`runtime.kind = "wasm-component"`) and exports the `yoi:plugin/instance@1.0.0` world.

It demonstrates both authoring surfaces supported by a shared Plugin instance:

- `example_echo` is an ordinary request/response Tool handler.
- `example_ws` is a Service ingress handler. The host owns WebSocket receive/reconnect work and dispatches bounded `websocket_text` events into `handle_ingress`. The guest replies by returning a `websocket_send` output command in `ServiceOutput`; do not run a guest-side `recv(timeout)` polling loop. The manifest declares `host_api.websocket` plus a matching `[[websocket]]` target for the example URL. Enablement grants must explicitly allow the same WebSocket target before the host will send output commands.

Build with `cargo component build --release` (or the project-specific build command used by your Plugin packaging flow), then run `yoi plugin check` / `yoi plugin pack` from the generated Plugin directory.
