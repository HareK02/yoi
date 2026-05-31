<!-- event: create author: tickets.sh at: 2026-05-31T02:28:21Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-31T02:29:17Z -->

## Decision

Decision note:

- `ListVisiblePods` should be folded into `ListPods` rather than kept as a separate tool.
- `InspectPod` is not needed as an LLM-facing tool; `ListPods` should provide enough state/detail, and action should go through restore/send/read/stop tools.
- `AttachOrRestorePod` should become `RestorePod`. The current "attach" branch only observes an already-live socket and returns status/socket data; it does not mean a persistent attachment or comm-registry materialization, so the name is misleading.
- If deeper semantics are needed later, define them explicitly as comm-registration/materialization rather than using "attach" implicitly.


---

<!-- event: implementation_report author: hare at: 2026-05-31T02:59:12Z -->

## Implementation report

Implementation report from coder Pod `pod-tool-surface-coder-20260531`:

- Branch: `pod-tool-surface-restore-list`
- Commit: `5472cec` (`pod: simplify pod tool surface`)
- Changed files: `crates/pod/src/controller.rs`, `crates/pod/src/discovery.rs`, `crates/pod/src/spawn/comm_tools.rs`, `crates/pod/src/spawn/registry.rs`, `crates/pod/src/spawn/tool.rs`, `crates/pod/tests/pod_comm_tools_test.rs`, `crates/protocol/src/lib.rs`, `crates/tui/src/app.rs`, `crates/tui/src/picker.rs`, `docs/architecture.md`.
- Simplified LLM-facing Pod tools to `ListPods` / `RestorePod`.
- Removed `ListVisiblePods`, `InspectPod`, and `AttachOrRestorePod` from active tool/protocol identifiers.
- Moved old `ListVisiblePods` state-backed visibility semantics into `ListPods`.
- Removed old spawned-registry-only `ListPods` comm tool and added `comm_registry` information to state-backed Pod listing output.
- `RestorePod` now reports already-live Pods or restores visible stopped/restorable Pods without using attach terminology.
- TUI protocol handling and architecture docs were updated.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo check -p pod -p protocol` passed with existing `llm-worker` warning only
- `cargo test -p protocol pod_discovery` passed
- `cargo test -p pod state_backed_visibility_and_restore_planning` passed
- `cargo test -p pod --test pod_comm_tools_test` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed

Unresolved issues: none.


---

<!-- event: review author: hare at: 2026-05-31T02:59:12Z status: approve -->

## Review: approve

External review by reviewer Pod `pod-tool-surface-reviewer-20260531`: approve.

Reviewer summary:

- `ListPods` is now the state/visibility-aware listing tool.
- `RestorePod` replaces old attach/restore naming.
- Active Rust source no longer contains `ListVisiblePods`, `InspectPod`, or `AttachOrRestorePod` tool/protocol identifiers.
- Visibility remains self + durable spawned children + live in-memory spawned registry, not host-wide enumeration.
- `RestorePod` rejects non-visible/missing/non-restorable Pods, reports already-live visible Pods, and only restores when state is restorable and unlocked.
- Existing `SpawnPod`, `SendToPod`, `ReadPodOutput`, and `StopPod` remain coherent; the old comm-registry-only `ListPods` tool was removed rather than retained as alias.
- TUI/protocol/docs were updated consistently.

Blockers: none.

Non-blocking follow-ups:

- `crates/pod/src/controller.rs` had a stale comment saying “four communication tools” after the comm tool count changed; not behavior-affecting.
- Run `cargo check -p tui` before merge because TUI files changed.
- A small assertion that removed tool names are absent from the registry would satisfy the ticket wording more literally, but reviewer did not consider it blocking.


---
