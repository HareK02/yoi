<!-- event: create author: LocalTicketBackend at: 2026-06-07T23:00:44Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: intake at: 2026-06-07T23:02:18Z -->

## Intake summary

Updated/verified existing Ticket as ready for Orchestrator routing. Intent: relax Intake/Planning/Orchestrator/Preflight workflow guidance so readiness is based on clear intent, reviewable acceptance criteria, explicit constraints, and escalation conditions, while preserving bounded coder implementation latitude. Acceptance criteria are observable across workflow wording, intent packet guidance, reviewer guidance, affected prompt/tests if any, and validation commands. needs_preflight: true because this changes workflow policy and orchestration semantics; risk_flags: workflow-policy, orchestration-semantics, reviewer-handoff, prompt-guidance. No duplicate Ticket was created.

---

<!-- event: state_changed author: intake at: 2026-06-07T23:02:18Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed for existing Ticket. Requirements and acceptance criteria are sufficient for Orchestrator routing; implementation must not start until the user queues the ready Ticket and Orchestrator accepts it.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-07T23:03:35Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-07T23:03:58Z -->

## Decision

Routing decision: preflight_needed

Reason:
- This Ticket intentionally changes workflow policy and orchestration semantics for Intake, Orchestrator routing, Preflight, and multi-agent coder/reviewer handoff.
- Intake explicitly marked `needs_preflight: true` with risk flags `workflow-policy`, `orchestration-semantics`, `reviewer-handoff`, and `prompt-guidance`.
- The desired direction is clear, but implementation should first fix the policy boundary: when bounded coder implementation latitude is acceptable, what remains a binding design/authority decision, and how reviewers should judge deviations.

Evidence checked:
- Ticket body requirements, non-goals, acceptance criteria, and affected workflow paths.
- Intake summary and `ready -> queued` event.
- Workspace state: no existing branch/worktree for this slug; an unrelated active child worktree/Pod exists for `parse-ticket-frontmatter-as-yaml`; unrelated untracked Ticket directories are present.
- Visible Pods: intake peer for this Ticket is idle; no coder/reviewer Pods are active for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record a concise wording/design boundary for:
  - implementation latitude versus binding decisions/invariants;
  - when `preflight_needed` remains mandatory;
  - what Orchestrator must put in IntentPacket as latitude and escalation conditions;
  - how reviewer instructions should judge against intent/constraints/acceptance rather than unrecorded tactics.
- Do not transition `queued -> inprogress`, create `.worktree/relax-implementation-planning-readiness`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- The wording would weaken authority/product/API/design-boundary gates rather than only narrowing unnecessary preflight churn.
- The change conflicts with current Ticket workflow state semantics or role-launch contracts.
- Prompt/resource changes are required beyond workflow docs and need separate validation scope.

---

<!-- event: decision author: hare at: 2026-06-07T23:08:01Z -->

## Decision

## Human decision: human-provided intent can satisfy preflight boundary for this policy cleanup

This Ticket itself exists to relax over-eager preflight/planning gates. The current Orchestrator routed it as `preflight_needed` mainly because Intake recorded `needs_preflight: true` and risk flags such as `workflow-policy`, `orchestration-semantics`, `reviewer-handoff`, and `prompt-guidance`.

That reaction is useful evidence for the problem this Ticket should fix: current guidance can mechanically classify workflow-policy changes as preflight-needed even when the human has already provided the intended boundary.

For this Ticket, additional preflight is not required before implementation if the implementation follows this human decision:

- Prioritize clarifying user/project intent over freezing implementation tactics early.
- Leave bounded implementation-investigation latitude to the coder when the intent, constraints, and acceptance criteria are clear enough for review.
- Reviewer should judge whether the implementation matches recorded intent, explicit constraints, acceptance criteria, and binding design decisions.
- If a design approach/invariant is explicitly recorded by the human, Orchestrator, or Ticket thread, the coder must follow it or escalate/record a justified deviation.
- Preflight remains mandatory when implementation would otherwise silently decide product/API/UX/authority-boundary questions or weaken explicit design constraints.
- `needs_preflight: true` and risk flags should be treated as strong signals, not unconditional hard blockers, when a human decision has already supplied the missing policy boundary.

Implementation should use this as the binding policy boundary and update workflow wording accordingly. Escalate only if the wording would weaken authority/product/API/design-boundary gates rather than narrowing unnecessary preflight churn.

---

<!-- event: decision author: orchestrator at: 2026-06-07T23:09:28Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Human recorded the missing policy boundary directly in the Ticket thread and explicitly authorized proceeding.
- The human decision clarifies that `needs_preflight: true` and risk flags are strong signals, not unconditional blockers, when the missing design/policy boundary has already been supplied.
- Intent, constraints, acceptance criteria, binding decisions, implementation latitude, and escalation conditions are now reviewable enough for coder/reviewer handoff.

Evidence checked:
- Ticket body requirements, non-goals, acceptance criteria, and affected workflow paths.
- Intake summary and prior `preflight_needed` routing decision.
- Human decision at `2026-06-07T23:08:01Z` supplying the binding policy boundary.
- Workspace state: main workspace clean; no branch/worktree exists for this slug; separate `parse-ticket-frontmatter-as-yaml` worktree/Pod is active but isolated.
- Visible Pods: intake peer for this Ticket is idle; no coder/reviewer Pods are active for this Ticket.

IntentPacket:

Intent:
- Relax workflow wording so Ticket readiness and implementation handoff focus on clear intent, constraints, acceptance criteria, and reviewability rather than prematurely fixed implementation tactics.

Binding decisions / invariants:
- Preflight is not removed; it remains mandatory when implementation would silently decide product/API/UX/authority-boundary questions or weaken explicit design constraints.
- Human/Orchestrator/Ticket-recorded design decisions and invariants remain binding. A coder must follow them or escalate/record a justified deviation.
- `ready` means ready for Orchestrator routing, not fully implementation-planned.
- `implementation_ready` may include bounded implementation uncertainty when intent, constraints, acceptance criteria, reviewer judgment basis, and escalation conditions are clear.
- `needs_preflight: true` and risk flags are strong signals, not unconditional hard blockers, if a human decision has already supplied the missing boundary.

Implementation latitude:
- The coder may choose exact wording and file-local organization after inspecting the workflow docs/prompts/tests.
- The coder may decide whether `.yoi/workflow/multi-agent-workflow.md` or role prompt/test wording needs updates, based on whether existing text over-demands pre-decided implementation plans.
- The coder may keep the change docs/workflow-only if no runtime prompt/tests are affected.

Requirements:
- Update `.yoi/workflow/ticket-intake-workflow.md`.
- Update `.yoi/workflow/ticket-orchestrator-routing.md`.
- Update `.yoi/workflow/ticket-preflight-workflow.md`.
- Update `.yoi/workflow/multi-agent-workflow.md` if needed.
- Adjust role prompt wording/tests if generated prompt guidance currently over-demands pre-decided implementation plans.
- Intent packet guidance must distinguish binding decisions/invariants, implementation latitude, and escalation conditions.
- Reviewer guidance must judge implementation against recorded intent, constraints, acceptance criteria, and explicit decisions—not unrecorded preferred tactics.
- Example wording should make coder investigation acceptable when intent is clear and risks are bounded.

Non-goals:
- Do not change Ticket state machine names.
- Do not weaken product/API/authority/design-boundary gates.
- Do not make coders silently decide product/API/authority questions.
- Do not introduce scheduler/lease/workflow-state redesign.

Escalate if:
- The wording would weaken authority/product/API/design-boundary gates rather than narrowing unnecessary preflight churn.
- A runtime prompt/resource/test change is larger than wording alignment.
- Existing workflow docs contain a contradictory policy that cannot be reconciled with this Ticket without broader redesign.

Validation:
- `target/debug/yoi ticket doctor` or equivalent built binary doctor.
- `cargo fmt --check`.
- `git diff --check`.
- If code, prompt runtime resources, or tests are changed, run focused tests and `nix build .#yoi`; if docs/workflow-only, report why Nix build was not needed.

Current code/docs map:
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`
- Search role prompts/tests only if workflow docs reveal generated guidance is out of sync.

Critical risks:
- Replacing too-strict preflight churn with too-loose implementation authority.
- Ambiguous reviewer instructions that evaluate against hidden preferences instead of recorded intent.
- Over-editing workflow docs beyond this policy cleanup.

---

<!-- event: state_changed author: orchestrator at: 2026-06-07T23:09:32Z from: queued to: inprogress reason: orchestrator_acceptance_after_human_policy_boundary field: workflow_state -->

## State changed

Orchestrator accepted the queued Ticket for implementation after the human decision supplied the policy boundary for bounded implementation latitude versus binding design/authority decisions. Worktree and sibling coder/reviewer routing may proceed from this accepted in-progress state.

---
