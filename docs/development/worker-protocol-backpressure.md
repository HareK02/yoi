# Worker operational protocol backpressure

The Workspace Worker operational connection is separate from Workspace request
admission (T-732). Both the dedicated Worker WebSocket and the Workspace
multiplexer use the identity-bound `WorkspaceWorkerMethodSender` and the same
remote/embedded bridge. This does not change Ticket assignment, general input
routing, or database execution placement.

## Ownership and admission

- Each bridge owns one captured remote WebSocket or embedded execution endpoint.
  It never follows a restored execution under the same Worker identity.
- Methods have 256 queue slots; events have 512. Browser authorization, input
  source attribution, interactive-Worker checks, and session exclusion remain at
  method admission. Queue admission is `try_send`, not a transport-capacity wait
  under the session guard. Success here means **queued**, not delivered/executed.
- Event enqueue never waits for its consumer. Snapshot is first; direct replies
  and live events retain the existing bridge ordering. No additional event pump
  can reorder them. Overflow ends the connection rather than skipping an event
  and continuing an uncertain stream.
- The first terminal cause wins: method overflow, event overflow/lag, transport
  timeout, or disconnect. Terminal state cannot reopen when capacity becomes
  available. It preempts pending browser admission and remote transport waits.
- The last client capability owns an abort-on-drop pump handle. The pump does not
  retain that capability, avoiding an ownership cycle. Event-consumer closure
  also cancels the pump even when method capabilities remain. Cancellation drops
  the bounded channels and captured transport; it does not stop the Worker.

A rejected full-queue method is known **not admitted to that queue**. Previously
queued methods, interrupted wire sends, and execution calls which have begun may
have unknown delivery or side effects. They are never converted to success or a
claim of non-execution, and are never automatically retried. Cancel is subject to
this same rule: it either enters the bounded queue or the connection explicitly
ends; there is no silent special-case drop or unbounded control lane.

## Transport deadlines and termination

Remote handshake and sends, browser writes (including Pong/error), and Workspace
multiplexer writes have 10-second deadlines. The dedicated browser session makes
one separately bounded Close attempt (at most another 10 seconds), with code 1011
and a short reason requiring snapshot resynchronization and warning of unknown
pending delivery. A completely blocked wire cannot guarantee delivery of the
Close/error frame; releasing the socket is the terminal signal in that case.

The multiplexer retains separate bounded control and high-volume protocol lanes
and control priority. Writer termination cancels the receive/admission loop, even
if that loop is awaiting control capacity or session ownership. Receive exit
aborts/joins the writer and subscription tasks instead of waiting indefinitely
for a slow writer to drain. Caller cancellation has abort-on-drop ownership.
Bridge overflow is exposed as `SubscriptionClosed { code: Lagged }`, not normal
resource disappearance, when the terminal control frame can be delivered.

Embedded dispatch is an existing synchronous Runtime API, not a new database or
executor redesign. Its connected-method adapter wait uses the existing
10-second cancellation-and-join waiter rather than leaving a detached send after
timeout. Existing completion and lifecycle ownership/fencing are retained. A
synchronous embedded dispatch already begun is not immediately preemptible by
browser disconnect; its bounded adapter wait settles before endpoint exclusion
is released. Cancellation/join is resource reclamation, **not rollback of an
already delivered method**.

## Recovery

Reconnect explicitly, resubscribe where using the multiplexer, and replace local
state with the fresh snapshot before live delivery (T-415). Do not replay old
method queues or transient protocol deltas. An ended connection never implicitly
Restores, reattaches, or retargets the Worker. Slow connections do not hold Server
session exclusion during transport waits, so other connections and Stop/session
cleanup can proceed.

## Evidence and limits

The initial static audit used `develop@a9a67fd6`; the same wait edges were present
at `develop@98023b98`. This is not evidence of a production incident. Controlled
fixtures exercise the actual remote connector, embedded bridge, browser method
admission/session, and Workspace multiplexer. Reinstating only the old bounded
queue waits makes the remote/embedded overflow and browser session-exclusion
fixtures time out; the corrected paths terminate without guessed sleeps.

Fixtures use channel/barrier ownership, controllable sinks, loopback WebSockets,
and paused time for transport deadlines. They cover ordered admitted prefixes,
auth rejection before admission, Cancel overflow, pending admission cancellation,
unknown wire delivery without retry, same-Worker fresh connection/Stop, and
transport/channel reclamation. They are not full browser/Runtime process E2E or a
production load benchmark. Existing attach/stop/restore and serialization tests
remain required alongside these regressions.

### Implementation validation (T-747)

- `cargo check --offline` at Workspace root: passed (normal TUI/Server closure).
- `cargo test -p yoi-workspace-server --offline --lib protocol_`: 29 passed,
  including the real bridges, browser admission/ownership, and stalled multiplexer.
- `cargo test -p yoi-workspace-server --offline`: 867 library tests passed, one
  existing ignored test, 11 binary tests passed, including the lag-code wire
  classification matrix (`--lib workspace_subscription::tests`: 4 passed).
- `cargo test -p worker-runtime --offline`: 416 library and 19 tests in each of
  the two binaries passed, including connected admission cancellation/join and
  existing endpoint fencing/lifecycle regressions.
- `cargo test -p protocol --offline`: 80 passed (wire contracts unchanged).
- `cargo fmt --all -- --check`, `git diff --check HEAD`, and
  `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts`:
  passed; naming checker reported 0 unapproved / 0 stale exceptions.

Additional Runtime compile probes with `--no-default-features`, with only
`http-server,fs-store`, and with only `ws-server` failed in pre-existing feature
boundaries (respectively missing gated request dependencies, missing futures
imports/dependency, and missing filesystem-backed retention methods). Replacing
only the changed Runtime file with its exact `98023b98` content produced the same
error groups in all three probes. These configurations are not claimed validated;
this patch does not repair their unrelated feature wiring. The normal
`ws-server,fs-store` configuration is covered by root check and whole-crate tests.
