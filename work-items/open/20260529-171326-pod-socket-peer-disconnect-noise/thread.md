<!-- event: create author: tickets.sh at: 2026-05-29T17:13:26Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-05-29T17:26:19Z status: approve -->

## Review: approve

Reviewed implementation commit `d5d50a3 fix: suppress pod socket peer disconnect noise` from branch `work/pod-socket-peer-disconnect-noise`.

Result: approved.

The implementation correctly separates normal peer disconnect read errors from invalid method payloads in `crates/pod/src/ipc/server.rs`:

- `ConnectionReset`, `ConnectionAborted`, `BrokenPipe`, and `UnexpectedEof` now close only the affected connection.
- Those peer disconnects no longer broadcast `Event::Error` to unrelated clients.
- malformed/schema-invalid Method lines still return a clear `InvalidRequest` error to the offending connection.

Focused tests cover both paths:

- `peer_disconnect_read_errors_are_connection_close`
- `invalid_data_is_not_peer_disconnect`
- `socket_schema_invalid_method_returns_error`
- `socket_malformed_method_returns_error`
- `socket_peer_close_without_method_does_not_broadcast_error`

Validation reported by implementer:

- `cargo fmt --check` passed
- `cargo test -p pod ipc::server -- --nocapture` passed
- `cargo test -p pod --test controller_test socket_ -- --nocapture` passed

Full `cargo test -p pod --test controller_test -- --nocapture` still has unrelated empty-turn rollback failures; the new socket tests passed within that run and the failing area is unrelated to this IPC disconnect change.


---
