<!-- event: create author: LocalTicketBackend at: 2026-06-08T03:29:11Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: intake at: 2026-06-08T03:31:05Z -->

## Decision

Intake readiness classification: `implementation_ready` for routing, with `needs_preflight: true`.

Rationale: the Ticket already defines the product boundary between `worker.language` and durable Ticket record language, names the expected config shape (`[ticket].language` preferred), preserves absent-config behavior, and lists observable acceptance criteria. Preflight is still required before implementation because the work touches Ticket config, role launch prompts, typed tool/backend generated text, profile/worker language boundaries, and generated durable records.

Risk flags: `config`, `profile-boundary`, `workflow-prompts`, `tool-generated-records`, `backward-compatibility`, `localization-policy`.

Implementation latitude: implementers may choose the minimal practical set of generation paths for the first change as long as they document the policy, preserve existing behavior when `ticket.language` is absent, avoid rewriting existing Tickets, and escalate before changing public/authority semantics beyond Ticket record language selection.

---

<!-- event: intake_summary author: intake at: 2026-06-08T03:31:10Z -->

## Intake summary

Existing Ticket refined and marked ready for Orchestrator routing. Scope: add explicit Ticket record language configuration separate from Pod/worker response language; expose it to role launch prompts and typed Ticket-generated record paths where practical; preserve absent-config behavior and avoid translating existing records or protocol literals. Readiness: implementation_ready for routing, with preflight recommended/required because the work touches config, profile/worker language boundaries, workflow prompts, and generated durable Ticket records. Risk flags: config, profile-boundary, workflow-prompts, tool-generated-records, backward-compatibility, localization-policy.

---

<!-- event: state_changed author: intake at: 2026-06-08T03:31:10Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed; Ticket has enough requirements, acceptance criteria, invariants, validation expectations, and escalation conditions for Orchestrator routing. Do not start implementation automatically; the next human/panel action is ready -> queued.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T05:51:29Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T06:03:40Z -->

## Decision

Routing decision: preflight_needed

Reason:
- This Ticket touches the boundary between worker response language, durable Ticket record language, role launch prompts, Ticket tools/workflows, memory policy, and public/operator-facing project records.
- The desired behavior is clear at a high level, but the exact source of truth for Ticket record language and how it is propagated into role launch/tool/workflow contexts needs a binding design note before coder delegation.
- Without preflight, implementation could accidentally conflate user-facing conversation language with durable Ticket record language, or create hidden context-only language instructions that conflict with history/prompt-cache policy.

Evidence checked:
- Ticket body and thread, including latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; active worktrees for direct/delegation authority and panel close are separate.
- Code/prompt context: this crosses Ticket backend/tool wording, role launch prompt generation, workflow templates, profile/worker language handling, and memory language policy.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: canonical config key/source for Ticket record language, fallback/default behavior, relationship to `[worker].language` and `[memory].language`, which Ticket tools/workflows/role launch prompts must use record language, whether existing Tickets are left as-is, how mixed-language user input is handled, tests/fixtures, and explicit non-use of hidden context-only language injection.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/separate-ticket-record-language-from-worker-language`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Ticket record language requires a migration of existing Ticket content.
- The design would make worker response language override durable Ticket record language.
- Language guidance would need to be injected into LLM context without being represented in committed history/tool/workflow prompts.
- The implementation would blur Ticket record authority with memory-language generation rules.

---
