<!-- event: create author: LocalTicketBackend at: 2026-06-07T23:45:30Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: Intake at: 2026-06-08T03:31:47Z -->

## Decision

## Intake refinement

Readiness: `implementation_ready`
Needs preflight: `true`
Risk flags: `pod-lifecycle`, `persistence`, `cli`, `panel`, `recovery`

### Binding decisions / invariants

- Fresh start must be explicit. Profile/workflow/system-guidance changes must not silently bypass normal restore behavior.
- Archive/reset targets only stopped/restorable Pod names. A live reachable Pod must be explicitly stopped before archive/reset is accepted.
- Preserve by default: archive/rename old metadata rather than destructively deleting it, with deterministic collision-safe archive naming.
- After archive, same-name launch should flow through the existing `missing -> spawn fresh` lifecycle rather than a hidden special-case restore bypass.
- User-facing docs/help should guide users to the official command/action and mention manual `~/.yoi/pods` surgery only as emergency/debug context.
- The implementation must be clear about what is preserved: Pod metadata/current-state and session logs/history should remain inspectable or recoverable where applicable.

### Implementation latitude

- The exact CLI spelling (`yoi pod archive <name>` versus `reset`/alias wording) can be chosen during design/preflight, provided the behavior and help text are explicit.
- Panel support may be a bounded Companion fresh-start action or diagnostic path; it does not need to become a general Pod manager if CLI coverage plus discoverability is sufficient.
- Archive storage may live under the Pod metadata root or a dedicated archive subdirectory if name lookup, collision handling, and recovery semantics remain simple.
- Tests can be focused around Pod metadata/lifecycle helpers and CLI/panel lifecycle behavior; full E2E process spawning is not required unless the implementation already has a suitable harness.

### Escalation conditions

Escalate before implementation if the chosen design would delete metadata/history by default, change live-Pod stop semantics, require a storage migration, introduce a new authority/scope boundary, or make Panel bypass restore without an explicit user action.

### Validation

Expected validation includes focused Rust tests for live refusal, stopped/restorable archive success, and same-name fresh launch behavior; `cargo fmt --check`; `git diff --check`; `target/debug/yoi ticket doctor`; and `nix build .#yoi` because this touches CLI/runtime/panel behavior.

---

<!-- event: intake_summary author: Intake at: 2026-06-08T03:31:53Z -->

## Intake summary

Existing Ticket refined for an explicit safe Pod archive/fresh-start path. It is implementation_ready with needs_preflight=true because it touches Pod lifecycle metadata/persistence plus CLI and Panel UX. Binding decisions: fresh start must be explicit, live reachable Pods are protected, archive preserves by default with deterministic collision-safe naming, same-name launch after archive uses the normal missing->spawn path, and docs should prefer official command/action over manual ~/.yoi/pods edits. Escalate if implementation would delete by default, alter live stop semantics, require migration, add authority/scope boundaries, or silently bypass restore.

---

<!-- event: state_changed author: Intake at: 2026-06-08T03:31:53Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement is complete. Ticket is ready for Orchestrator routing; implementation must not start until the user queues it and Orchestrator accepts it as inprogress.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T03:48:46Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T03:49:14Z -->

## Decision

Routing decision: preflight_needed

Reason:
- Intake marked this Ticket `implementation_ready` but also explicitly `needs_preflight=true` for Pod lifecycle metadata/persistence plus CLI and Panel UX.
- Several product/storage choices are still intentionally left open: command spelling (`archive`/`reset`), archive location/layout, deterministic collision naming, recovery semantics, and whether Panel support is a bounded Companion action or diagnostic path.
- Those choices affect user-facing CLI, metadata preservation, live/restorable Pod safety, and recovery expectations, so coder delegation should wait until a short binding design note fixes the boundary.

Evidence checked:
- Ticket body: background, requirements, acceptance criteria.
- Thread: Intake refinement, binding decisions/invariants, implementation latitude, escalation conditions, validation, and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; main workspace has unrelated Ticket-record edits for other Intake work.
- Visible Pods: only peer Intake Pods and this Orchestrator are live; no coder/reviewer implementation Pods are active.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: final CLI spelling and help text, archive storage layout, deterministic collision-safe naming format, exact live-Pod refusal/stop interaction, what metadata/history is moved or preserved, how same-name launch observes `missing -> spawn`, Panel Companion UX scope, recovery/inspection wording, and focused test strategy.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/add-pod-archive-fresh-start-path`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- The design would delete metadata/history by default.
- The implementation would alter live-Pod stop semantics or silently bypass restore because profiles/workflows changed.
- Archive requires a storage migration or a new authority boundary.
- Panel support would become a broad Pod manager rather than a bounded explicit fresh-start path.

---

<!-- event: state_changed author: hare at: 2026-06-10T09:29:55Z from: planning to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-10T09:29:55Z status: closed -->

## 完了

現時点では fresh-start-path を独立機能として実装しない方針に変更する。問題の実体は Pod 名の restorable metadata 衝突と古い Pod/session 履歴の蓄積であり、fresh start 専用 UX ではなく、Pod 名を明示的に空ける archive/delete と storage cleanup/prune の設計で扱う。既存の通常 lifecycle（restorable -> restore、missing -> spawn）は維持し、restore bypass の専用 path は作らない。必要になれば Pod/session storage cleanup commands として別 Ticket を切る。


---
