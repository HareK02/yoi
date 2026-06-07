<!-- event: create author: LocalTicketBackend at: 2026-06-07T22:32:33Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: intake at: 2026-06-07T22:36:44Z -->

## Intake summary

Existing Ticket is sufficiently specified and not a duplicate of another visible active Ticket. Intake classifies it as implementation_ready with needs_preflight=true because it changes durable Ticket frontmatter parsing and compatibility semantics. Risk flags: ticket-backend, yaml-parsing, compatibility, panel-regression. Scope is limited to YAML/null-aware Ticket item.md parsing, list/bool handling, preserving emitted format/doctor compatibility, and the workspace panel attention_required:null regression. Validation should include focused ticket parser tests, workspace_panel regression tests, cargo fmt --check, git diff --check, and relevant cargo test commands; final implementation should also consider the project-level nix build requirement.

---

<!-- event: state_changed author: intake at: 2026-06-07T22:36:44Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed: requirements, acceptance criteria, non-goals, affected fields, and validation are explicit enough for Orchestrator routing. Human queueing remains required before implementation starts.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-07T22:43:03Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-07T22:43:24Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is well scoped and urgent, but it changes durable Ticket frontmatter parsing and compatibility semantics.
- Intake explicitly classified it as implementation-ready only with `needs_preflight=true`, with risk flags `ticket-backend`, `yaml-parsing`, `compatibility`, and `panel-regression` in the intake summary.
- YAML/null-aware parsing affects nullable strings, lists, booleans, doctor compatibility, existing fixture behavior, and workspace panel action derivation; a short design boundary check should precede coder delegation.

Evidence checked:
- Ticket body requirements, candidate affected fields, non-goals, and acceptance criteria.
- Intake summary and `ready -> queued` event.
- Workspace state: `develop` has only this Ticket's queued item/thread modifications plus an unrelated untracked Ticket directory `20260607-224309-reduce-ticket-lifecycle-commit-noise`; no branch/worktree exists for this slug.
- Visible Pods: intake peer for this Ticket is idle; no coder/reviewer Pods are active for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should decide the parsing boundary: whether to deserialize `item.md` frontmatter through `serde_yaml::Value`/typed helpers or a narrower YAML-compatible layer; how to preserve emitted format; how to represent quoted literal `"null"` versus YAML null; and which tests cover Ticket parser plus workspace panel regression.
- Do not transition `queued -> inprogress`, create `.worktree/parse-ticket-frontmatter-as-yaml`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Existing Ticket records rely on raw-string parsing semantics that conflict with YAML behavior.
- Adding a YAML parser dependency has packaging/licensing implications not already acceptable for this crate graph.
- Fixing panel action derivation requires broader Ticket workflow/action model changes beyond parsing correctness.

---
