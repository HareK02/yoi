<!-- event: create author: LocalTicketBackend at: 2026-06-08T01:56:30Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: intake at: 2026-06-08T02:40:51Z -->

## Intake summary

Existing Ticket refined enough for Orchestrator routing. Request is an implementation-ready bug fix: make client::PodClient own/cancel its background Unix-socket reader task on drop so short-lived panel probes do not leak file descriptors. Acceptance focuses on Drop-triggered reader abort, preserving normal event receive behavior while alive, regression coverage for repeated connect/drop cleanup, and validation via focused client tests plus formatting/diff/ticket doctor checks. needs_preflight: false; risk_flags: [async-cancellation, pod-client-lifecycle, fd-leak].

---

<!-- event: state_changed author: intake at: 2026-06-08T02:40:51Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake marked ready: requirements, acceptance criteria, implementation boundary, validation, and escalation surface are clear enough for Orchestrator routing. No implementation is started by Intake.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T02:40:58Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T02:41:57Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Intake marked this as ready with `needs_preflight: false`; the Ticket describes a concrete bug and a narrow lifecycle fix.
- Requirements and acceptance criteria are observable: `PodClient` must own/cancel its background reader task, dropping the client must abort the reader and release the read half promptly, and existing send/receive behavior must remain intact.
- The relevant code map is narrow: `crates/client/src/pod_client.rs` owns `PodClient::connect`, the spawned reader task, send/recv methods, and existing client tests; `crates/tui/src/pod_list.rs` exercises repeated short-lived live status probes.
- Remaining uncertainty is bounded async test design, not a product/API/authority boundary decision.

Evidence checked:
- Ticket body: background, goal, requirements, acceptance criteria.
- Thread: intake summary and `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; main has unrelated Ticket-record changes for `allow-spawnpod-child-workspace-cwd` that are understood and outside the implementation paths.
- Code map search for `PodClient`, `PodClient::connect`, reader task spawn, live status probing, and `try_connect_live_pod`.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Fix the `PodClient` reader-task lifecycle so short-lived clients used by panel/live Pod probing do not leak Unix socket file descriptors.

Binding decisions / invariants:
- `PodClient` owns the lifetime of the background reader task created by `PodClient::connect()`.
- Dropping `PodClient` must cancel/abort the reader task without relying on remote socket close.
- Aborting the reader task must drop the read half of the Unix socket promptly.
- While `PodClient` is alive, normal event receive behavior must continue to work.
- Existing one-shot clients, TUI clients, Ticket role launcher communication, and Pod-management tools must continue using `PodClient` normally.
- Do not change Pod socket protocol, event format, controller behavior, registry semantics, or panel polling policy unless a focused test shows a minimal caller adjustment is necessary.

Requirements / acceptance criteria:
- Store the reader task `JoinHandle` in `PodClient` or provide equivalent owned cancellation.
- Implement `Drop` for `PodClient` to abort/cancel the reader task.
- Preserve `send`, `try_recv_event`, and `recv_event` behavior while alive.
- Add focused regression coverage proving drop aborts/cleans up the reader task or otherwise closes repeated connect/drop clients without leaking stuck reader tasks.
- Add/adjust live status probe tests if needed.
- Avoid brittle FD-count-only tests if a deterministic task/connection cleanup assertion is more robust.

Implementation latitude:
- Coder may choose exact field names, test seams, and whether to expose test-only observability through existing module tests.
- Coder may use server-side connection closure or task completion probes in tests if they deterministically prove the dropped client no longer holds the socket read half.
- Coder may add a small helper if it improves readability, but should avoid broad refactors.

Escalate if:
- Fixing reader-task cleanup requires changing `PodClient` public API or Pod socket protocol.
- Drop-based cancellation conflicts with long-lived TUI event consumption.
- Deterministic regression coverage is impossible without invasive test-only hooks.
- The observed FD leak appears to come from a different source after inspecting `PodClient`.

Validation:
- Focused client tests for `pod_client` / reader-task drop behavior.
- Relevant TUI live status probe tests if changed.
- `cargo test -p client ... --lib` or focused equivalent selected by coder.
- `cargo test -p tui pod_list` if status probing is touched.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- If runtime/client behavior is touched, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/client/src/pod_client.rs`: `PodClient` struct, `connect`, spawned reader task, send/recv tests.
- `crates/tui/src/pod_list.rs`: repeated `PodClient::connect` live status probing and existing probe tests.
- `crates/tui/src/single_pod.rs` and `crates/client/src/ticket_role.rs`: long-lived / one-shot client users to preserve conceptually.

Critical risks / reviewer focus:
- Dropping `PodClient` must abort the reader task even if the server stays open and silent.
- The fix must not drop or abort the reader while a live client is still expected to receive events.
- Regression tests should fail against the old behavior rather than only checking that a field exists.
- Avoid masking leaks by relying only on remote close or timeout behavior.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T02:42:04Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and `PodClient` code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---
