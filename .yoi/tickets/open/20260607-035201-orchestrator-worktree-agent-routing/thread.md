<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:52:01Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T05:39:34Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready as the second Orchestrator automation slice, after `orchestrator-queued-ticket-routing` landed.

Intent:
- Enable the workspace Orchestrator to route an accepted `inprogress` Ticket through the existing worktree + coder/reviewer sibling workflow.
- Treat `worktree-workflow` and `multi-agent-workflow` as the operational contract to be builtin/role guidance for Orchestrator behavior.
- Stop at a merge-ready dossier; do not merge/close in this ticket.

Prerequisite state:
- `orchestrator-queued-ticket-routing` now makes Panel Queue notify Orchestrator and requires `queued -> inprogress` before implementation side effects.
- Ticket role launch config strict validation and `yoi ticket init` scaffold are in place.
- Ticket lifecycle feature access levels are in place.
- The local role session registry exists for local Pod/session association.

Requirements for this slice:
- Orchestrator must only create worktrees/spawn coder/reviewer after the Ticket is already `inprogress`.
- Use `worktree-workflow` for mechanical worktree rules:
  - `.worktree/<task-name>`;
  - child worktree excludes `.yoi`;
  - main workspace remains authority for Ticket/workflow/docs records.
- Use `multi-agent-workflow` for sibling coder/reviewer loop:
  - coder and reviewer are siblings under Orchestrator;
  - coder gets narrow write scope to child worktree;
  - reviewer is read-only by default;
  - coder gets intent packet, worktree/branch, validation, report expectations;
  - reviewer gets Ticket intent/diff/validation and blocker/non-blocker criteria.
- Record durable progress in Ticket thread without prematurely recording main-branch approval:
  - allowed: worktree plan, coder delegated/completed/blocked, reviewer delegated, blocker/fix-loop summaries;
  - branch-local reviewer verdict stays in merge-ready dossier/review report until merge-completion phase.
- Produce a merge-ready dossier for `orchestrator-merge-completion` after review approval and validation.

Likely implementation surfaces:
- `.yoi/workflow/ticket-orchestrator-routing.md`, `.yoi/workflow/worktree-workflow.md`, `.yoi/workflow/multi-agent-workflow.md` for role/workflow guidance. Child worktree excludes `.yoi`; any workflow edits must be reported for parent-side application.
- `crates/client/src/ticket_role.rs` already supports coder/reviewer launch context with worktree path, branch, intent packet, validation, and report expectations.
- `crates/tui/src/multi_pod.rs` / panel runtime may already handle Orchestrator notifications; avoid adding full scheduler behavior here unless a small clear routing hook exists.
- Worktree creation may remain prompt/workflow-driven for first pass; do not build a broad worktree manager unless clearly necessary.

Non-goals:
- Do not implement Queue notification/acceptance; already done.
- Do not implement merge/cleanup/close; `orchestrator-merge-completion` owns it.
- Do not add dynamic state-machine tool gating.
- Do not record reviewer approve as a final main Ticket approval before merge.
- Do not let coder Pods edit main workspace `.yoi` or Ticket records.

Validation:
- Add/update prompt/resource tests if available; otherwise add focused unit tests around any helper/prompt text changed.
- Run focused tests for changed crates, plus `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` if Ticket/workflow files are touched.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T05:39:48Z -->

## Intake summary

Implementation-ready as the second Orchestrator automation slice: after `queued -> inprogress` acceptance, route accepted Tickets through `worktree-workflow` and `multi-agent-workflow` as builtin/role guidance, spawn coder/reviewer sibling Pods with correct scopes, record progress durably, and stop at a merge-ready dossier without merge/close.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T05:39:48Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---
