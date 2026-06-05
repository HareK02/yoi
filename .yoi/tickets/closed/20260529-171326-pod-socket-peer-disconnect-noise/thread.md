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

<!-- event: close author: hare at: 2026-05-29T17:26:50Z status: closed -->

## Closed

---
id: 20260529-171326-pod-socket-peer-disconnect-noise
slug: pod-socket-peer-disconnect-noise
title: Treat Pod socket peer disconnects as normal connection close
status: closed
kind: bug
priority: P1
labels: [pod, ipc, tui, noise]
created_at: 2026-05-29T17:13:26Z
updated_at: 2026-05-29T17:26:50Z
assignee: null
legacy_ticket: null
---

## Background

The active `insomnia` TUI session frequently shows notices like:

```text
[notice error] pod: [Internal] invalid method: Connection reset by peer (os error 104)
```

The message is misleading. The Pod IPC server reports every `reader.next::<Method>()` error as an `Event::Error` with `invalid method: ...`. A peer disconnect/reset from a short-lived socket client or status probe is a normal connection lifecycle event, not an invalid method sent by a client.

This becomes noisy during orchestration, multi-Pod polling, attach/picker refreshes, and diagnostic tooling because many clients connect briefly to read initial `Alert` / `Snapshot` traffic and then close. Depending on timing, the server can observe that close as `ConnectionReset`, `ConnectionAborted`, `BrokenPipe`, or EOF-like errors. Those must not be broadcast as user-visible Pod errors.

Likely code path:

- `crates/pod/src/ipc/server.rs`: `handle_connection`, `method = reader.next::<Method>()`
- `crates/protocol/src/stream.rs`: JSON line reader returns I/O errors
- TUI displays broadcast `Event::Error` through notice/error surfaces

## Requirements

- Classify normal peer disconnects while reading client `Method` values as connection close, not invalid method.
  - At minimum handle `ConnectionReset`, `ConnectionAborted`, `BrokenPipe`, and EOF-like cases appropriately.
  - The handler should break/return for those cases without broadcasting `Event::Error`.
- Preserve diagnostics for genuinely invalid client input.
  - Malformed JSON or schema-invalid `Method` payloads should still produce a clear error.
  - Prefer sending the protocol error to the offending connection only if the current IPC shape makes that reasonable; do not unnecessarily broadcast connection-local parse errors to unrelated TUI clients.
- Keep normal socket behavior intact.
  - Initial `Alert` and `Snapshot` delivery must still work.
  - Long-lived TUI attach clients must still receive live events.
  - Short-lived probe clients may connect, read enough state, and drop without generating user-visible errors.
- Avoid hiding real server failures.
  - Only expected peer disconnect/read-close errors should be silenced.
  - Unexpected I/O or parse errors should remain observable through logs or explicit error events as appropriate.
- Add focused tests.
  - A client that connects and resets/closes without sending a Method should not create an `Event::Error` / notice.
  - A malformed Method line should still be treated as invalid input.
  - Existing controller/IPC tests should continue to pass.

## Acceptance criteria

- The `[Internal] invalid method: Connection reset by peer` notice no longer appears when short-lived Pod socket clients/probes disconnect.
- Normal disconnect/reset/abort/broken-pipe while reading a Method closes only that connection and is not broadcast to all clients.
- Invalid JSON or schema-invalid Method input still produces a clear diagnostic.
- Tests cover peer disconnect handling and invalid-method handling.
- `cargo fmt --check`
- Relevant pod/protocol/tui IPC tests pass.


---
