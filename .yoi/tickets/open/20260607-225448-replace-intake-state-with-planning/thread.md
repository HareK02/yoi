<!-- event: create author: LocalTicketBackend at: 2026-06-07T22:54:48Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T07:07:59Z -->

## Decision

## Decision: remove standalone `preflight` concept; use return-to-planning

Do not preserve `preflight` as a separate workflow concept, operation, or long-lived routing bucket.

The desired model is:

- Intake/Planning prepares a Ticket until it can become `ready`.
- Orchestrator reads queued/ready work and may find that implementation cannot be planned safely yet.
- In that case, Orchestrator records the missing information/decision and returns the Ticket to `planning` with a visible reason.
- There is no separate `preflight_needed` state, no separate preflight lane, and no Intake-authored `needs_preflight` gate.

Responsibility split:

- Intake/Planning owns requirements clarification and Ticket shaping.
- Orchestrator owns routing and may reject/return work to planning when the Ticket lacks required decisions, constraints, or implementation intent.
- Planning owns resolving that returned reason.

Workflow/documentation implications:

- Remove `needs_preflight` from Intake-facing concepts. Intake may record `risk_flags`, open questions, and readiness, but should not decide that a future Orchestrator preflight operation is required.
- Replace Orchestrator `preflight_needed` classification with a planning-return classification such as `planning_needed` / `return_to_planning` / `requirements_sync_needed`.
- Remove or fold `ticket-preflight-workflow` into the planning workflow/docs. The useful content is the checklist for deciding whether a Ticket has enough binding decisions and implementation intent; it should not be exposed as a separate operation/lane.
- Risk flags are reviewer/orchestrator attention markers, not stop gates.
- If Orchestrator cannot name a concrete missing decision/information item, it should not return the Ticket to planning merely because the area is risky; it should proceed with an IntentPacket plus escalation/reviewer focus.

Acceptance impact:

- The implementation should update workflow-state terminology, prompts/workflows, panel labels/actions, and tests so missing implementation readiness is represented as a return to `planning`, not as `preflight_needed`.
- Existing mentions of `preflight_needed` should be removed, renamed, or treated as migration/legacy compatibility only where needed.

---

<!-- event: intake_summary author: ticket-intake-workflow at: 2026-06-08T07:22:47Z -->

## Intake summary

Existing Ticket was refined as an implementation-ready planning-state replacement task. The binding direction is to replace the user-facing `intake` workflow state with a planning-oriented state/lane, preserve the Intake role name as separate terminology, and remove standalone `preflight` / `needs_preflight` concepts from the user-facing workflow. Orchestrator missing-readiness routing should record a concrete reason and return the Ticket to planning, not create a separate preflight lane. Implementation should update state parsing/normalization or migration, transition graph, panel labels/actions, role/workflow prompts/docs, Ticket tools/CLI surfaces, and tests. Risk flags for routing/review attention: workflow-state, migration, panel-ux, orchestrator-routing, prompt-workflow-docs. No separate preflight gate remains; risk flags are attention markers, not blockers.

---

<!-- event: state_changed author: ticket-intake-workflow at: 2026-06-08T07:22:47Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket has enough binding decisions, invariants, acceptance criteria, and validation expectations for Orchestrator routing. Marking ready for user queueing; implementation must not start until the user/panel performs `ready -> queued` and Orchestrator accepts it.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T08:35:07Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-08T08:37:14Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body and thread contain the binding product decision: replace the user-facing `intake` workflow state/lane with a planning-oriented state and remove standalone `preflight` / `needs_preflight` concepts.
- The Intake summary explicitly marks this as implementation-ready and separates the Intake Pod role name from workflow-state terminology.
- Remaining uncertainty is implementation scope management across model parsing, transition graph, Panel labels/actions, routing/workflow prompts, docs, and tests; these are bounded engineering tasks with clear reviewer criteria.

Evidence checked:
- Ticket body requirements and acceptance criteria.
- Thread decision: remove standalone `preflight` concept and use return-to-planning.
- Thread intake summary and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; only this Ticket record is dirty from queue/intake updates.
- Code/workflow map search for `TicketWorkflowState`, `workflow_state`, `intake`, `planning`, `preflight_needed`, `needs_preflight`, transition graph, Panel derivation, and workflow docs.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Replace user-facing Ticket workflow state `intake` with a planning-oriented state/lane and make Orchestrator missing-readiness routing return Tickets visibly to planning rather than using standalone preflight concepts.

Binding decisions / invariants:
- Use a planning-oriented workflow state; prefer `planning` unless implementation finds a stronger concise name.
- Keep the Intake role/profile/persona name separate from workflow state naming.
- Do not preserve `preflight` as a separate workflow state, lane, or long-lived operation.
- Remove Intake-authored `needs_preflight` as a stop gate; risk flags/open questions remain attention markers.
- Orchestrator may return `queued` or `ready` Tickets to `planning` only with a concrete missing decision/information reason.
- If Orchestrator cannot name a concrete missing item, it should not return solely because the area is risky; it should proceed with IntentPacket plus escalation/reviewer focus.
- Preserve auditability through typed state-change/routing events.
- Do not redesign implementation worktree/coder/reviewer mechanics except where routing returns to planning.
- Existing `workflow_state: intake` must be handled compatibly and normalized/emitted according to a clear policy.

Requirements / acceptance criteria:
- Replace `TicketWorkflowState::Intake` user-facing/model output with planning-oriented state.
- Define and implement legacy `intake` parsing/migration behavior.
- Update transition graph to `planning -> ready -> queued -> inprogress -> done` and support return-to-planning from `ready`/`queued` with reason.
- Update Ticket tools, CLI, Panel labels/actions, prompts/workflows/docs, and tests that treat `intake` as workflow state.
- Remove or fold standalone `preflight_needed` routing/workflow language into planning return / requirements-sync language.
- Panel should show planning/clarification/preflight-preparation terminology and launch path when no claimed Planning/Intake Pod exists.
- If a claimed live/restorable Intake/Planning Pod exists, Orchestrator return-to-planning should notify/send it the reason where a suitable existing path exists or add a clear follow-up if not practical in the first implementation.
- Tests must cover state parsing/normalization, transition graph, Panel action derivation, and return-to-planning behavior.

Implementation latitude:
- Coder may implement legacy `intake` as an accepted input alias normalized to `planning`, or migrate fixtures/records where safe; avoid broad unrelated Ticket rewrites.
- Coder may keep internal role strings and Pod names containing `intake` where they refer to the Intake role, not workflow state.
- Coder may stage removal of `ticket-preflight-workflow` by updating active references/guidance and leaving compatibility files only if removing them would break workflow discovery unexpectedly; report the boundary.
- Coder may choose exact helper names and test split.

Escalate if:
- Removing `needs_preflight` requires a storage migration beyond typed metadata compatibility.
- Workflow discovery cannot tolerate removing/renaming `ticket-preflight-workflow` without a separate migration.
- Notifying claimed Planning/Intake Pods requires a new durable relation/notification mechanism rather than existing local role-session claims and peer notification paths.
- Parser normalization would make existing Tickets ambiguous or doctor-invalid without a broad migration.

Validation:
- Ticket crate tests for workflow-state parsing/normalization and transition graph.
- Tool tests for workflow transitions, including return-to-planning from ready/queued and stale transition rejection.
- Panel tests for planning labels/actions and no heuristic fallback to intake.
- Prompt/workflow/doc search ensuring `preflight_needed` / `needs_preflight` are removed or explicitly marked legacy compatibility.
- Focused CLI/tool tests selected by coder.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because Ticket schema/tools/Panel/prompts/workflows are touched, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/ticket/src/lib.rs` and `crates/ticket/src/tool.rs`: `TicketWorkflowState`, transition graph, tools, doctor/tests.
- `crates/tui/src/workspace_panel.rs` and `crates/tui/src/multi_pod.rs`: Panel row derivation/actions and queue/return flows.
- `crates/yoi/src/ticket_cli.rs`: CLI parsing/display/scaffold tests.
- `crates/client/src/ticket_role.rs`: role launch guidance and terminology.
- `.yoi/workflow/ticket-intake-workflow.md`, `.yoi/workflow/ticket-orchestrator-routing.md`, `.yoi/workflow/ticket-preflight-workflow.md`, `.yoi/workflow/multi-agent-workflow.md`: active workflow guidance.
- `resources/prompts` if active prompt text refers to workflow-state intake/preflight.

Critical risks / reviewer focus:
- Do not confuse Intake role identity with planning workflow state.
- Legacy `workflow_state: intake` must not break existing Tickets.
- Return-to-planning must require a concrete reason and remain auditable.
- Removing preflight language must not remove the useful checklist for missing binding decisions/invariants; it should be folded into planning/readiness guidance.
- Panel must not infer state from labels/readiness/needs_preflight heuristics.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T08:37:22Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and workflow-state code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T08:37:56Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `replace-intake-state-with-planning`
- Worktree: `.worktree/replace-intake-state-with-planning`
- Base: `cb234b8 ticket: route planning workflow-state replacement`
- Scope: replace user-facing workflow-state `intake` with planning terminology, remove standalone preflight/needs_preflight routing concepts, and update tests/docs/prompts/panel/tool behavior.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/replace-intake-state-with-planning`.
- Coder should run commands from the child worktree; current live Orchestrator may not yet expose the newly merged `SpawnPod.cwd` schema until restart, so the task packet still includes explicit `cd` instructions.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T09:02:18Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-replace-intake-planning`
- Commit: `ada6db9 feat: replace intake workflow state with planning`
- Worktree status: clean after commit.
- Diff against `develop`: 11 files changed, 290 insertions, 270 deletions.

Files touched by coder:
- `.yoi/workflow/multi-agent-workflow.md`
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `crates/client/src/ticket_role.rs`
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/workspace_panel.rs`
- `docs/development/work-items.md`
- `docs/development/workflows.md`

Coder reported implementation:
- Replaced user-facing/model-output `TicketWorkflowState::Intake` with `Planning`.
- Legacy `workflow_state: intake` parses as `Planning`; emitted/default state is `planning`.
- Updated transition graph to `planning -> ready -> queued -> inprogress -> done`, and allowed reasoned `ready/queued -> planning` returns through typed state-change events.
- Preserved Intake role/tool names where they refer to role identity (`TicketIntakeReady`, Intake role/profile/persona).
- Removed new `needs_preflight` input from `TicketCreate`; legacy metadata remains for read compatibility and is no longer a stop gate.
- Updated Panel terminology/actions toward planning/clarification/preparation.
- Kept `ticket-preflight-workflow.md` as compatibility slug, but rewrote it toward planning / requirements sync rather than standalone preflight state/lane.

Coder reported validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p ticket --lib`
- `cargo test -p ticket workflow_state --lib`
- `cargo test -p ticket ticket_workflow_tool --lib`
- `cargo test -p ticket ticket_intake_ready_tool --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p yoi ticket_cli`
- `cargo check --workspace`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-replace-intake-planning`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by current live launch behavior.
- Reviewer was instructed to judge against the recorded Ticket requirements and binding decisions.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred for this Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T09:09:28Z -->

## Implementation report

Reviewer returned `request_changes`; coder fix-loop delegated.

Reviewer verdict:
- `request_changes`

Blockers:
1. Workflow docs reference a non-existent planning-sync workflow slug:
   - `.yoi/workflow/multi-agent-workflow.md`
   - `.yoi/workflow/ticket-intake-workflow.md`
   - references such as `ticket-planning sync-workflow` / `$user/ticket-planning sync-workflow` have no matching workflow file and contain a space inside the apparent slug.
   - Since `ticket-preflight-workflow.md` remains as compatibility slug rewritten toward planning/requirements sync, references should use the actual compatibility slug or a deliberate new real slug.
2. Maintained docs still present “Preflight” as a normal lifecycle gate:
   - `docs/development/work-items.md` still says “Ticket Intake, Orchestrator Routing, Preflight, and Multi-agent workflows” and “Preflight or spike when needed.”
   - This conflicts with the binding decision to not preserve standalone Preflight as a workflow state/lane/long-lived gate.

Reviewer non-blocking observations:
- Rust implementation and focused tests look sound.
- Existing tracked Ticket records containing `workflow_state: intake` are acceptable because legacy parse normalizes to `Planning` and doctor passes.
- `needs_preflight` remaining as legacy metadata/model output is acceptable if it is no longer a public tool input/stop gate.
- “Ticket Intake” wording remains acceptable where it means role/persona rather than workflow state.

Action taken:
- Sent fix-loop instructions to `coder-replace-intake-planning` with the workflow slug and docs wording blockers.

Pending:
- Await coder fix report and reviewer re-review.
- No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T09:14:49Z -->

## Implementation report

Coder fix-loop completed and reviewer re-review delegated.

Coder follow-up:
- Commit: `eddb33c docs: fix planning sync workflow references`
- Worktree: clean on branch `replace-intake-state-with-planning`.
- Fixes claimed:
  - Replaced invalid/non-existent planning-sync workflow references with the actual compatibility slug `ticket-preflight-workflow` / `$user/ticket-preflight-workflow`.
  - Clarified that `ticket-preflight-workflow` is now a planning/requirements-sync compatibility entry point rather than standalone Preflight state/lane.
  - Updated `docs/development/work-items.md` to remove Preflight as an ordinary lifecycle gate and use planning/requirements-sync wording.
  - Focused searches no longer find `ticket-planning`, `planning sync-workflow`, `Preflight`, or `preflight_needed`; remaining `preflight` / `needs_preflight` references are reported as legacy compatibility/metadata only.

Coder validation reported:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p ticket --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p yoi ticket_cli`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Action taken:
- Sent updated commits and prior blocker checklist to `reviewer-replace-intake-planning` for read-only re-review.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred.

---
