<!-- event: create author: LocalTicketBackend at: 2026-06-08T07:17:22Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: intake at: 2026-06-08T10:58:22Z -->

## Plan

Intake refinement: implementation_ready

Context checked:
- Target Ticket body/thread.
- `replace-intake-state-with-planning` resolution: planning-state model is settled and `ready|queued -> planning` now requires audited typed state-change reasons.
- `allow-spawnpod-child-workspace-cwd` resolution: confirms the motivating authority-adjacent example was risky but sufficiently specified and should be treated as reviewer focus when binding decisions exist.
- Current `.yoi/workflow/ticket-orchestrator-routing.md`: already contains the routing surface to refine, including `return_to_planning`, `implementation_ready`, `Evidence checked`, and IntentPacket sections.

Binding decisions / invariants:
- Returning a ready/queued Ticket to `planning` must require a concrete missing decision/information item after bounded relevant context checking.
- Risk flags and risky domains are context lookup triggers and reviewer-focus signals, not stop gates.
- If no concrete gap can be named after context checking, risky-but-specified Tickets should route as `implementation_ready` when other criteria are satisfied.
- The routing decision record must expose checked evidence and explain why any gap cannot be safely handled as implementation latitude.
- Do not reintroduce a separate readiness/preflight lane or operation.

Implementation latitude:
- Implementation may choose whether to refine only `.yoi/workflow/ticket-orchestrator-routing.md`, related prompt resources, focused tests, or all of those if the current code layout requires it.
- Wording and test structure may be chosen locally as long as the policy is explicit and reviewer can verify the acceptance criteria.

Escalation conditions:
- Escalate if implementation would change the workflow-state graph or Ticket lifecycle semantics beyond the documented `planning -> ready -> queued -> inprogress -> done` model.
- Escalate if enforcing evidence recording requires new public API/tool schema changes rather than workflow/prompt/test updates.
- Escalate if the existing prompt/workflow guidance is already fully compliant and no meaningful code/resource change is needed; record a close-ready/no-op dossier instead of manufacturing churn.

Validation:
- `target/debug/yoi ticket doctor`
- `git diff --check`
- Focused prompt/workflow validation or tests covering: risky but specified Ticket -> implementation-ready; genuinely missing decision -> return to planning; risk flag alone is insufficient as the reason.
- `nix build .#yoi` if runtime resources, prompts, code, or packaging inputs change.

---

<!-- event: intake_summary author: intake at: 2026-06-08T10:58:29Z -->

## Intake summary

既存 Ticket を重複作成せず refinement した。`replace-intake-state-with-planning` の完了により前提は満たされており、対象 Ticket は Orchestrator routing guidance の bounded context check policy として implementation_ready。binding invariants、implementation latitude、escalation conditions、validation を thread に補足済み。未決定の blocking question はない。risk flags は workflow/prompt、orchestrator-routing、planning-state、authority-boundary/context-check。

---

<!-- event: state_changed author: intake at: 2026-06-08T10:58:29Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement completed. Ticket has enough intent, acceptance criteria, binding invariants, implementation latitude, escalation conditions, and validation guidance for Orchestrator routing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T00:04:21Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-09T00:06:25Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, current Orchestrator routing workflow guidance, workspace state, visible Pod/worktree state, and the recently merged orchestration-plan/tool changes. No active implementation worktree is present; the requested change is bounded to routing prompt/workflow guidance and tests/resources.

---

<!-- event: decision author: orchestrator at: 2026-06-09T00:06:25Z -->

## Decision

Routing decision: implementation_ready

Evidence checked:
- Ticket body and acceptance criteria for `orchestrator-return-to-planning-context-policy`.
- Current `.yoi/workflow/ticket-orchestrator-routing.md`, including existing `return_to_planning`, `implementation_ready`, and queued acceptance sections.
- Current workspace/worktree state after merging and cleaning up the previous TUI/composer work.
- Visible Pod state: no active sibling coder/reviewer for earlier work remains.
- Recent merged context: `replace-intake-state-with-planning`, `ticket-orchestration-plan-tool`, and queued-routing behavior already require explicit queued acceptance and durable evidence.
- `TicketOrchestrationPlanQuery` is documented in the workflow, but this current orchestrator session's tool list does not expose the newly merged tool, so there was no callable plan-query surface to inspect for this Ticket.

Reason:
- The Ticket has concrete requirements and acceptance criteria.
- The change is policy/prompt/workflow guidance, not a broad runtime redesign.
- Current workflow text already contains some related clauses, so implementation should be a bounded tightening: make the evidence/context requirement explicit, ensure risky-but-specified Tickets can proceed, and add/adjust focused validation.

IntentPacket:

Intent:
- Strengthen Orchestrator routing guidance so returning ready/queued Tickets to `planning` requires concrete missing information/decision evidence after bounded project-context checks, not just Ticket text or risk flags.

Binding decisions / invariants:
- Do not reintroduce a separate readiness/preflight state or lane.
- Do not change Ticket workflow states or transition semantics.
- Do not make every queued Ticket require broad repository archaeology.
- Risk flags and risky domains are context lookup and reviewer-focus signals, not automatic stop gates.
- If returning to planning, the routing record must include the concrete missing decision/information, context checked, why implementation latitude is insufficient, and the next planning question/action.
- If no concrete missing item remains after bounded context checks, a risky-but-specified Ticket should proceed as implementation_ready with IntentPacket invariants, escalation conditions, and reviewer focus.
- Keep the change in maintained routing workflow/prompt/resource docs and focused validation; avoid runtime scheduler/orchestration refactors.

Implementation latitude:
- Coder may update `.yoi/workflow/ticket-orchestrator-routing.md`, resource prompts/workflows, and tests that validate workflow/prompt guidance.
- If there is no existing prompt-resource test harness, coder may add a narrow text validation test in the most appropriate existing crate, or document why focused grep/resource validation is the right evidence.
- Include or reference an example equivalent to `allow-spawnpod-child-workspace-cwd`: authority-adjacent but specified work should proceed with reviewer focus rather than return to planning solely because it touches authority/scope/Pod concerns.

Escalate if:
- Implementation requires changing Ticket backend workflow_state semantics.
- Existing tests/resources imply a conflicting policy that must be resolved by human decision.
- A broad prompt/workflow templating redesign would be needed instead of a narrow guidance update.

Validation:
- Focused workflow/prompt validation or tests covering: risky but specified Ticket -> implementation-ready; genuinely missing decision -> return to planning; risk flag alone is insufficient.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- `cargo check --workspace` if code/tests are touched.
- `nix build .#yoi` if resources/tests/package inputs are touched.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T00:07:10Z -->

## Implementation report

Implementation routing started.

Worktree/branch:
- Worktree: `.worktree/orchestrator-return-to-planning-context-policy`
- Branch: `orchestrator-return-to-planning-context-policy`
- Base/routing commit: `9f35442 ticket: route return-to-planning policy`

Spawned sibling Coder Pod:
- `coder-orchestrator-return-to-planning-context-policy`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Routing context:
- The previous composer-history implementation was merged, validated, closed, and cleaned up before this new queued Ticket was accepted.
- This Ticket is now the active routed implementation work.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T00:19:47Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-orchestrator-return-to-planning-context-policy`
- Commit: `8576615 workflow: tighten orchestrator planning return policy`
- Worktree status before review: clean branch `orchestrator-return-to-planning-context-policy`
- Stopped after collecting output to reclaim delegated worktree scope.

Implementation summary:
- Tightened Orchestrator routing workflow so returning `ready` / `queued` Tickets to `planning` requires concrete missing decision/information evidence after bounded project-context checks.
- Clarified risk flags / risky domains / authority-adjacent work as context lookup, IntentPacket invariant, reviewer-focus, and escalation signals rather than automatic stop gates.
- Added risky-but-specified guidance and an `allow-spawnpod-child-workspace-cwd`-style example for proceeding with reviewer focus when work is specified.
- Updated related planning/preflight and multi-agent workflow wording plus work-item docs.

Changed files:
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`
- `docs/development/work-items.md`

Coder validation reported passed:
- Focused workflow/prompt text validation for risky-but-specified -> implementation_ready, genuinely missing decision -> return_to_planning, and risk flag alone insufficient.
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Coder did not run `cargo check --workspace` because only docs/workflow resources changed.

---

<!-- event: review author: reviewer at: 2026-06-09T00:23:32Z status: approve -->

## Review: approve

Review result: approve

Scope reviewed:
- Commit `8576615` (`workflow: tighten orchestrator planning return policy`).
- Changed files: `.yoi/workflow/ticket-orchestrator-routing.md`, `.yoi/workflow/ticket-preflight-workflow.md`, `.yoi/workflow/multi-agent-workflow.md`, `docs/development/work-items.md`.
- Ticket body/thread and orchestration plan records for this Ticket (`TicketOrchestrationPlanQuery`: 0 records).
- Targeted stale-guidance search across `.yoi/workflow`, `docs`, and `resources/prompts` for preflight/planning/risk/return-to-planning terminology.

Findings:
- No separate readiness/preflight state or lane is reintroduced. `ticket-orchestrator-routing.md` keeps workflow_state as `planning -> ready -> queued -> inprogress -> done` and describes `ticket-preflight-workflow` only as a legacy planning/requirements-sync slug; `ticket-preflight-workflow.md` explicitly says not to treat `preflight` as a state/lane/current metadata vocabulary.
- Ticket workflow-state and queued acceptance semantics remain unchanged. The updated guidance still requires `queued -> inprogress` before worktree/spawn/implementation side effects and does not add a new transition.
- Returning `ready`/`queued` Tickets to `planning` now requires concrete missing decision/information plus checked context, why implementation latitude is insufficient, and next planning question/action.
- Risk flags/risky domains are consistently documented as bounded context lookup, IntentPacket/reviewer focus, and escalation signals rather than automatic stop gates.
- The guidance stays bounded: it names targeted context sources and explicitly says the purpose is not broad repository archaeology.
- Risky-but-specified work is allowed to proceed as `implementation_ready`; the routing examples include the required `allow-spawnpod-child-workspace-cwd`-equivalent authority/scope/Pod example with reviewer focus rather than planning return solely for risk category.
- Maintained workflow/docs are aligned. I did not find conflicting authority-adjacent automatic-stop wording in `.yoi/workflow`, `docs`, or `resources/prompts` searches.

Validation run in the worktree:
- Focused text validation via grep assertions for risky-but-specified -> `implementation_ready`, concrete missing item requirements, risk-alone insufficiency, bounded context/non-archaeology wording, preflight compatibility wording, multi-agent proceed-with-reviewer-focus wording, and work-items docs alignment.
- `git diff --check 8576615^ 8576615` passed.
- `cargo run -q -p yoi -- ticket doctor` passed (`doctor: ok`).
- `nix build .#yoi` passed.
- Worktree remained clean after review.

I did not run `cargo check --workspace`; the diff is workflow/docs text only, and `nix build .#yoi` plus Ticket doctor covered packaging/runtime-resource integrity for this change.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T00:24:31Z -->

## Implementation report

Merge-ready dossier for human/orchestrator approval.

Status:
- Implementation branch: `orchestrator-return-to-planning-context-policy`
- Worktree: `.worktree/orchestrator-return-to-planning-context-policy`
- Commit under review: `8576615 workflow: tighten orchestrator planning return policy`
- Reviewer: `reviewer-orchestrator-return-to-planning-context-policy`
- Review result: approved; no blocking findings.

Intent / invariant check:
- Strengthens Orchestrator routing guidance so `ready` / `queued` Tickets are returned to `planning` only when a concrete missing decision/information remains after bounded project-context checks.
- Keeps workflow_state semantics unchanged: `planning -> ready -> queued -> inprogress -> done`.
- Does not reintroduce preflight/readiness as a separate state/lane.
- Keeps risk flags and authority-adjacent domains as context lookup / reviewer-focus / escalation signals, not automatic stop gates.
- Allows risky-but-specified Tickets to proceed as `implementation_ready` with IntentPacket invariants, escalation conditions, and reviewer focus.

Implementation summary:
- Updated `ticket-orchestrator-routing` to require concrete missing-item evidence, checked context, implementation-latitude insufficiency, and next planning action before planning return.
- Added an `allow-spawnpod-child-workspace-cwd`-style authority/scope/Pod example showing specified risky work proceeding with reviewer focus.
- Aligned `ticket-preflight-workflow`, `multi-agent-workflow`, and work-item docs with the same policy.

Validation evidence:
- Coder reported pass:
  - focused workflow/prompt text validation for risky-but-specified -> implementation_ready, genuinely missing decision -> return_to_planning, and risk flag alone insufficient
  - `git diff --check`
  - `cargo run -q -p yoi -- ticket doctor`
  - `nix build .#yoi`
- Reviewer independently passed:
  - focused grep/text validation across `.yoi/workflow`, `docs`, and `resources/prompts`
  - `git diff --check 8576615^ 8576615`
  - `cargo run -q -p yoi -- ticket doctor`
  - `nix build .#yoi`

Residual risks / notes:
- `cargo check --workspace` was intentionally not run by coder/reviewer because the implementation is workflow/docs text only; `nix build .#yoi` and Ticket doctor were used for packaging/resource integrity.
- Final merge/close/cleanup is intentionally not performed here without explicit merge approval.

---

<!-- event: state_changed author: hare at: 2026-06-09T01:18:52Z from: inprogress to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state を done に設定しました。


---

<!-- event: close author: hare at: 2026-06-09T01:18:52Z status: closed -->

## 完了

Implemented, reviewed, merged, and validated.

Summary:
- Strengthened Orchestrator routing guidance so `ready` / `queued` Tickets are returned to `planning` only when a concrete missing decision/information remains after bounded project-context checks.
- Clarified risk flags, risky domains, and authority-adjacent work as context lookup / reviewer-focus / escalation signals rather than automatic stop gates.
- Added risky-but-specified guidance and an `allow-spawnpod-child-workspace-cwd`-style example for proceeding with reviewer focus when work is specified.
- Aligned `ticket-preflight-workflow`, `multi-agent-workflow`, and work-item docs with the same policy.
- Preserved workflow_state semantics and did not reintroduce preflight/readiness as a separate state or lane.

Implementation:
- Coder commit: `8576615 workflow: tighten orchestrator planning return policy`
- Reviewer approved with no blocking findings.
- Merge commit: `5af58b5 merge: tighten orchestrator planning return policy`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

`cargo check --workspace` was intentionally omitted because the merged implementation is workflow/docs text only and no Rust code/tests changed; Nix build covered resource/package integrity.

---
