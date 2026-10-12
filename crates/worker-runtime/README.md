# worker-runtime

Manages Worker creation, stopping, and Restore, execution connections within the Runtime, and Workdir materialization. The Backend owns Workspace records and grants; the Worker controller owns model execution and input acceptance. Optional features provide HTTP/WebSocket and filesystem persistence.

## Reference index

- [`Runtime`]: lifecycle operation acceptance, Workspace boundaries, and execution connections.
- [`execution`] — execution backends and operation-specific results.
- [`working_directory`] — repository materialization, session resources, and safe deletion.
- [`workspace_issuer`] — Workspace signature verification and replay prevention.
