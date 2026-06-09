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
