<!-- event: create author: LocalTicketBackend at: 2026-06-08T05:45:46Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-08T06:18:12Z -->

## Intake summary

Implementation-ready: implement post-idle self-shutdown for Ticket Intake role Pods after a successful `TicketIntakeReady`, preserving tool result/final response/history commits and leaving local Ticket claim/session records intact. needs_preflight=true due to Pod lifecycle, session/history ordering, role/session detection, and panel/local-claim behavior. risk_flags=[pod-lifecycle, session-history, ticket-workflow, panel-claim].

---

<!-- event: state_changed author: ticket-intake at: 2026-06-08T06:18:12Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake complete; Ticket has explicit goal, requirements, ordering constraints, invariants, acceptance criteria, validation expectations, and escalation risks for Orchestrator routing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T06:20:43Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T06:21:07Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is well scoped, but Intake explicitly identified `needs_preflight=true` due to Pod lifecycle, session/history ordering, role/session detection, and panel/local-claim behavior.
- The required shutdown must happen after tool result delivery, assistant final response, history/session commits, and the Idle transition. That ordering crosses tool-result handling, Worker/Pod lifecycle state, controller shutdown, and role-session metadata boundaries.
- The design must also fix which component owns the transient `shutdown_after_idle` signal and how Intake-role detection is performed without injecting hidden model-visible instructions.

Evidence checked:
- Ticket body: background, requirements, design considerations, acceptance criteria.
- Thread: Intake summary and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; active `split-direct-and-delegation-authority` worktree is separate.
- Visible Pods: many completed Intake peer Pods remain idle/live, which is the behavior this Ticket aims to address; no implementation Pod exists for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: exact lifecycle hook/owner for the shutdown-after-idle flag, success signal source for `TicketIntakeReady`, role/session detection source, ordering relative to tool result/history commit/assistant response/Idle, behavior if another user turn arrives before shutdown, failure diagnostics, claim preservation expectations, and focused test strategy.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/shutdown-intake-pod-after-ready-idle`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Shutdown-after-idle cannot be represented as transient runtime state without hidden context injection.
- Role/session detection for Intake Pods is not durable enough to distinguish Companion/Orchestrator/Coder/Reviewer callers.
- Clean shutdown after Idle conflicts with current controller/session commit ordering.
- Preserving local claims while stopping the Pod requires changes to claim semantics rather than lifecycle state only.

---

<!-- event: decision author: hare at: 2026-06-08T07:35:00Z -->

## Decision

## Routing clarification: not a human-blocking preflight case

This Ticket's intended behavior is specific enough to route toward implementation:

```text
Ticket Intake Pod successfully calls TicketIntakeReady
  -> mark that Intake Pod for shutdown-after-idle
  -> allow tool result, assistant final response, history/session commits, and turn cleanup to complete
  -> when the Pod returns to Idle, cleanly shut it down
  -> preserve the local Ticket claim/session record
```

The important invariants are already stated in the Ticket body:

- do not shut down during the tool call;
- do not kill the Pod before the final assistant response/history commit is preserved;
- perform shutdown only after the Pod returns to Idle;
- only actual Ticket Intake role/session Pods are affected;
- failed `TicketIntakeReady` calls do not schedule shutdown;
- Companion/Orchestrator/Coder/Reviewer Pods do not self-shutdown merely because they invoked a Ticket tool;
- local Ticket claim/session records remain for audit/restore;
- the signal should be transient runtime state, not hidden model-visible instruction text.

The previous `preflight_needed` routing decision should be interpreted as an implementation-planning concern under the older vocabulary, not as a reason to wait for a new human requirement decision. The remaining questions are primarily code-map / implementation-intent questions:

- which Pod/controller lifecycle hook owns the transient `shutdown_after_idle` flag;
- where to observe successful `TicketIntakeReady` tool results;
- which role/session metadata source identifies actual Ticket Intake Pods;
- how to order shutdown after tool result delivery, assistant response, history/session commit, turn cleanup, and Idle transition;
- what happens if another user turn arrives before the post-idle shutdown runs;
- how bounded diagnostics are surfaced if scheduling or shutdown fails;
- how tests cover success, failure, role mismatch, and Idle ordering.

Preferred routing under the planning-oriented model:

- do not treat Pod lifecycle/session-history risk flags as a stop gate by themselves;
- Orchestrator should inspect the relevant current code paths, record a concise implementation intent/code map, then accept the Ticket if no concrete missing product/API/lifecycle decision is found;
- if code inspection reveals a genuine missing decision that cannot be handled as implementation latitude, return to planning with that concrete missing decision and checked evidence;
- otherwise proceed to implementation with invariants, escalation conditions, and reviewer focus in the IntentPacket.

---

<!-- event: decision author: orchestrator at: 2026-06-08T07:37:13Z -->

## Decision

Routing decision: implementation_ready

Correction:
- The previous `preflight_needed` decision should be treated as implementation-planning caution, not a human-blocking gate. The human clarification records that the required lifecycle invariants are already fixed by the Ticket body.

Reason:
- The behavior is specific and observable: successful `TicketIntakeReady` from an actual Ticket Intake Pod schedules post-idle shutdown; failed calls and non-Intake roles do not.
- Ordering constraints are explicit: tool result delivery, final assistant response, history/session commits, and turn cleanup must complete before shutdown.
- Claim/session semantics are explicit: local Ticket claim remains for audit/restore; this work stops the live Pod, not the claim.
- Remaining uncertainty is bounded code-map/implementation design: where to observe successful tool results, how to identify Intake role/session, and which Pod/controller Idle hook performs the clean shutdown.

Evidence checked:
- Ticket body requirements, design considerations, and acceptance criteria.
- Thread clarification by `hare` that this is not a human-blocking preflight case and lists the binding invariants.
- Workspace state: no matching branch/worktree exists; active `allow-spawnpod-child-workspace-cwd` worktree is separate and currently in fix-loop.
- Code map search: `TicketIntakeReady` built-in feature registration, Pod hooks/post-tool-call surfaces, Worker tool result callbacks, Controller status/Idle/shutdown handling, protocol `Shutdown`/`PodStatus::Idle`, and TUI role-session/panel claim display paths.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Automatically stop a Ticket Intake Pod after it successfully completes `TicketIntakeReady`, but only after its current turn has fully settled and the Pod is Idle.

Binding decisions / invariants:
- Only actual Ticket Intake role/session Pods are eligible.
- Successful `TicketIntakeReady` schedules shutdown-after-idle; failed tool results do not.
- Companion, Orchestrator, Coder, Reviewer, and arbitrary Pods must not self-shutdown merely because they invoke Ticket tools.
- Shutdown must not happen during the tool call or before tool result delivery, final assistant response, history/session commits, and turn cleanup are complete.
- The signal is transient runtime state, not model-visible instruction text and not persisted as a durable Ticket/claim mutation.
- Local Ticket claim/session records remain intact; stopping the Pod must not release/delete the claim.
- Use the ordinary clean Pod shutdown lifecycle where practical.
- If another user turn arrives before the queued shutdown executes, prefer deterministic safe behavior: do not interrupt an active turn; either run shutdown only once the Pod is Idle again or cancel/ignore according to a clearly tested local rule.

Requirements / acceptance criteria:
- Observe successful `TicketIntakeReady` tool result in a Pod-side or controller-side path with enough context to know current Pod role/session.
- Record a transient `shutdown_after_idle` flag/request only for eligible Intake Pods.
- Execute clean shutdown after the Pod transitions back to Idle following the successful turn.
- Preserve final assistant response and history/session commits before shutdown.
- Failed `TicketIntakeReady` does not schedule shutdown.
- Non-Intake role/session callers do not schedule shutdown.
- Panel/local claim behavior remains claim-preserving and should show stopped/restorable where existing metadata allows.
- Add focused tests for success, failure, role mismatch, and ordering around Idle transition.

Implementation latitude:
- Coder may choose whether the transient flag lives in Controller, SharedState, Pod runtime state, or a narrow feature/hook adapter, as long as it is not model-visible and not durable claim state.
- Coder may use existing hook/post-tool-call metadata or Worker `on_tool_result` callback if that gives reliable success/failure and tool-name evidence.
- Coder may use existing Pod metadata/Profile role/session fields for Intake detection, or add a small explicit runtime marker if current durable metadata is insufficient.
- Coder may implement deterministic behavior for a new user turn before shutdown as either defer-until-next-idle or cancel, but must document and test it.

Escalate if:
- There is no reliable runtime source for the current Pod's Ticket Intake role/session identity.
- Successful `TicketIntakeReady` cannot be distinguished from failed result in available hook/callback metadata.
- Clean shutdown after Idle conflicts with the current controller method loop/session commit ordering.
- Preserving claims while stopping the Pod would require changing role-session claim semantics.

Validation:
- Focused Pod/controller/feature tests for shutdown-after-idle scheduling and execution.
- Tests for failed tool result and role mismatch not scheduling shutdown.
- Tests/probes for ordering: final response/history commit path completes before shutdown event/method closes the Pod.
- TUI/panel claim tests only if panel display logic changes.
- `cargo test -p pod ...` focused equivalents selected by coder.
- `cargo test -p tui role_session_registry --lib` or panel tests only if touched.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because Pod lifecycle/runtime behavior is touched, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/pod/src/feature/builtin/ticket.rs`: Ticket tool registration/feature boundary for `TicketIntakeReady`.
- `crates/pod/src/hook.rs` and feature hook plumbing: post-tool-call hook surfaces.
- `crates/llm-worker/src/worker.rs` / `interceptor.rs`: tool result callback/interceptor evidence path.
- `crates/pod/src/controller.rs`: Pod status transitions, Idle handling, `Method::Shutdown`, socket/controller shutdown lifecycle.
- `crates/pod/src/pod.rs`: Worker run handling, history/session commit ordering, hook registration, role/profile/runtime metadata access.
- `crates/protocol/src/lib.rs`: `PodStatus::Idle`, `Method::Shutdown`, `Event::Shutdown` semantics.
- `crates/tui/src/role_session_registry.rs` and `workspace_panel.rs`: local claim/session display expectations; should remain unchanged unless a focused test requires it.

Critical risks / reviewer focus:
- Shutdown must not truncate the assistant's final response or skip history/session persistence.
- Non-Intake roles must not be affected.
- Failed `TicketIntakeReady` must not schedule shutdown.
- The shutdown request must be transient runtime state, not hidden prompt/context injection.
- Claim/session records must remain for audit/restore.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T07:37:20Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after human clarification and routing IntentPacket were recorded. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---
