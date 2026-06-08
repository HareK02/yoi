<!-- event: create author: LocalTicketBackend at: 2026-06-07T22:02:25Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: Intake at: 2026-06-08T02:41:28Z -->

## Decision

## Intake refinement

The existing Ticket is sufficiently specified for Orchestrator routing and implementation planning.

- readiness: implementation_ready
- needs_preflight: true
- risk_flags: [persistence, launch-lifecycle, ticket-claims, panel-intake, peer-registration]

Binding decisions / invariants for implementation:

- Durable local Intake claims must represent confirmed launched/restorable Intake Pods, not speculative launch attempts.
- Prefer post-success claim commit over rollback-heavy pre-claiming.
- Duplicate prevention while launch is in flight must use transient/non-durable state, not a durable claim that can survive a failed launch.
- Existing live/restorable claims must continue to prevent duplicate Intake launch.
- Existing stale claims remain diagnostic/manual-reclaim territory unless a claim can be safely identified as created by the current failed launch attempt.
- Preserve Ticket Intake handoff peer registration where possible.
- Do not change Ticket workflow-state semantics, Orchestrator lifecycle, or Companion routing in this work.

Implementation latitude remains: choose the smallest local mechanism for transient in-flight protection and test seams, provided the acceptance criteria cover failed launch before durable claim commit and successful launch claim creation.

---

<!-- event: intake_summary author: Intake at: 2026-06-08T02:41:34Z -->

## Intake summary

Existing bug Ticket refined and marked ready. The requested behavior is to commit local Intake Ticket claims only after launch success/acceptance evidence, avoid durable speculative pre-claims, keep successful existing-Ticket Intake launches claiming exactly once, preserve live/restorable duplicate prevention, and leave unrelated stale-claim cleanup as manual/diagnostic. Readiness is implementation_ready; needs_preflight is true because the work touches Pod launch lifecycle, local role-session claim persistence, panel Intake behavior, and handoff/peer-registration boundaries. Suggested risk flags: persistence, launch-lifecycle, ticket-claims, panel-intake, peer-registration.

---

<!-- event: state_changed author: Intake at: 2026-06-08T02:41:34Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed for existing Ticket; Orchestrator can route it. Do not start implementation until the user queues it from ready to queued.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T03:06:07Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T03:08:19Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The Ticket and Intake thread now contain binding decisions/invariants for the launch-claim lifecycle, so the remaining work is bounded implementation and test design.
- Although Intake marked `needs_preflight: true`, the recorded refinement already fixes the key persistence/launch-lifecycle boundaries: durable claims must represent confirmed launched/restorable Intake Pods, in-flight duplicate prevention must be transient, existing live/restorable claims continue to block, and existing stale claims remain diagnostic/manual-reclaim territory.
- The relevant code map is local enough for coder delegation: role-session claim persistence in `role_session_registry.rs`, existing-Ticket Intake preparation/finish flow in `multi_pod.rs`, panel claim display in `workspace_panel.rs`, and Ticket role launch handoff/peer registration surfaces.

Evidence checked:
- Ticket body requirements and acceptance criteria.
- Thread: Intake refinement binding decisions, intake summary, and `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; main has unrelated Ticket-record edits and a separate `allow-spawnpod-child-workspace-cwd` preflight decision, both understood and outside the implementation paths.
- Code map search for `claim_ticket`, role-session registry, existing Ticket Intake launch preparation, launch finish, handoff/peer registration, and local claim display.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Ensure existing-Ticket Intake local claims are durable only after launch success/acceptance, so failed first launches do not leave ghost/stale claims that block retries.

Binding decisions / invariants:
- Durable local Intake claims must represent confirmed launched/restorable Intake Pods, not speculative launch attempts.
- Prefer post-success claim commit over rollback-heavy pre-claiming.
- Duplicate prevention while launch is in flight must use transient/non-durable state if needed.
- Successful existing-Ticket Intake launch must still write exactly one local claim and role-session record.
- Existing live/restorable claims must continue to prevent duplicate Intake launch and direct the user to the existing Pod.
- Existing stale claims remain diagnostic/manual-reclaim territory unless the implementation can safely identify a claim created by the current failed launch attempt.
- Preserve Ticket Intake handoff peer registration where possible.
- Do not change Ticket workflow-state semantics, Orchestrator lifecycle, Companion routing, or unrelated stale-claim cleanup policy.

Requirements / acceptance criteria:
- Do not write a durable local Ticket claim until Intake Pod launch has succeeded with sufficient acceptance evidence; at minimum spawn/connect/initial `Method::Run` acceptance should complete before durable claim commit.
- Failed launch for a previously unclaimed Ticket must leave no local claim file/record for that Ticket.
- Retrying after failed launch must not be blocked by a ghost/stale claim from that failed attempt.
- Successful launch must write exactly one claim for the launched Pod.
- Existing live/restorable claims must continue to block duplicate Intake launch.
- Tests should cover failure before spawn, failure after spawn/connect or initial Run rejection/timeout if practical, and successful launch.

Implementation latitude:
- Coder may choose the smallest local transient in-flight guard/test seam.
- Coder may restructure the existing pre-claim flow into post-success claim commit, or keep a pre-claim shape only if rollback is reliable and simple; post-success is preferred.
- Coder may add focused unit tests around `MultiPodApp`/registry state rather than E2E launch tests if the launch boundary is mocked through existing launch result types.

Escalate if:
- Claim commit cannot be delayed until launch acceptance without changing Ticket role launcher APIs broadly.
- Peer registration/handoff ordering conflicts with post-success claim commit.
- Existing live/restorable duplicate prevention requires durable pre-claim semantics.
- Test coverage would need invasive runtime/E2E infrastructure beyond focused TUI/registry seams.

Validation:
- Focused TUI/registry tests for existing-Ticket Intake claim timing.
- `cargo test -p tui role_session_registry --lib` if registry tests are added/changed.
- `cargo test -p tui multi_pod --lib` or focused equivalent selected by coder.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because TUI launch/claim behavior is touched, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/tui/src/role_session_registry.rs`: durable local role-session registry and `claim_ticket` persistence.
- `crates/tui/src/multi_pod.rs`: `prepare_existing_ticket_intake_launch`, `launch_intake_with_handoff`, `finish_intake_launch`, `IntakeLaunchRequest`, `IntakeLaunchResult`, peer registration/handoff flow, related tests.
- `crates/tui/src/workspace_panel.rs`: local claim display/status and existing claim duplicate UX.
- `crates/client/src/ticket_role.rs`: launch/acceptance boundary to preserve conceptually if touched.

Critical risks / reviewer focus:
- No durable claim may be left after simulated launch failure for a previously unclaimed Ticket.
- Successful launch must still claim exactly once.
- Existing live/restorable claims must still block duplicate launch.
- Transient in-flight guard must not become a durable stale claim.
- Peer registration/handoff behavior should not regress silently.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T03:08:26Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and Intake claim lifecycle code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T03:08:56Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `commit-intake-claims-after-launch-success`
- Worktree: `.worktree/commit-intake-claims-after-launch-success`
- Base: `d2a040d ticket: route intake claim and spawn cwd tickets`
- Scope: implement post-success existing-Ticket Intake claim commit and focused tests.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/commit-intake-claims-after-launch-success`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Related queued/preflight note:
- `allow-spawnpod-child-workspace-cwd` was routed as `preflight_needed` and remains queued; do not depend on the future `SpawnPod.cwd` field for this implementation.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---
