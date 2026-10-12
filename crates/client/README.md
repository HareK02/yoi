# client

Shared client for Backend Workspace/Runtime APIs and the Worker protocol. Provides authenticated communication and typed connection targets for CLI/TUI. The standalone crate owns Standalone execution, Runtime and the controller own Worker state and input admission, and higher-level crates own presentation and product commands.

## Implementation entry points

- [`BackendApiClient`] — Authenticated HTTP/WS requests bound to an origin
- [`backend_runtime`] / [`backend_workspace`] — Backend resource retrieval and operations
- [`Client`] / [`transport`] — Method/Event encoding and message transport
- [`target`] — Distinct Backend and Standalone targets and operation intents
