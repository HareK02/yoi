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

<!-- event: implementation_report author: hare at: 2026-06-07T05:40:37Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `orchestrator-agent-routing-coder-20260607` in worktree `.worktree/orchestrator-worktree-agent-routing` on branch `work/orchestrator-worktree-agent-routing`.

Scope:
- Add builtin/role guidance or focused implementation wiring for accepted `inprogress` Tickets to proceed through worktree + coder/reviewer sibling routing.
- Follow `worktree-workflow` and `multi-agent-workflow` contracts.
- Preserve `.yoi` exclusion in child worktrees and main-workspace authority for Ticket/workflow records.
- Stop at merge-ready dossier; do not implement merge/close.
- Keep branch-local reviewer verdicts out of final main Ticket approval until merge-completion.

Workflow note:
- Child worktree excludes `.yoi`; if workflow file updates are needed, the child should report an exact parent-side patch rather than editing `.yoi`.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, workflow file updates, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T05:49:40Z -->

## Implementation report

## Implementation report

Coder Pod `orchestrator-agent-routing-coder-20260607` completed implementation in `.worktree/orchestrator-worktree-agent-routing`.

Commit:
- `c7d6bb8 orchestrator: add agent routing guidance`

Reported summary:
- Added generated role guidance in `crates/client/src/ticket_role.rs`:
  - Orchestrator: route only after `workflow_state = inprogress`, use `worktree-workflow` and `multi-agent-workflow`, record bounded Ticket-thread progress, and stop at merge-ready dossier.
  - Coder: operate only in child worktree/branch, avoid main `.yoi`/Ticket/workflow/docs records, report validation/status, and do not merge/close/delete.
  - Reviewer: read-only by default, review branch diff/validation against intent, keep branch-local verdict in dossier/report, and do not record final main approval.
- Expanded `crates/tui/src/multi_pod.rs` Queue notification guidance to mention the post-acceptance worktree/coder/reviewer sibling route and stopping at merge-ready dossier.
- Added prompt-generation and queue-notification tests for these contracts.
- Did not edit/copy `.yoi` workflow files in the child worktree.

Reported validation:
- `cargo test -p client ticket_role -- --nocapture`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract -- --nocapture`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- child-built `yoi ticket doctor` against the main workspace
- `nix build .#yoi`

Parent-side workflow patch needed: none reported.

---

<!-- event: review author: orchestrator-agent-routing-reviewer-20260607 at: 2026-06-07T05:55:11Z status: approve -->

## Review: approve

Approved.

Evidence:
- Role-specific guidance in `crates/client/src/ticket_role.rs` gates worktree/Pod side effects on `workflow_state = inprogress`, names `worktree-workflow` and `multi-agent-workflow`, excludes `.yoi`, keeps Ticket/workflow/docs/memory authority in the main workspace, and stops at a merge-ready dossier.
- Coder guidance limits work to the child worktree/branch, forbids main-workspace `.yoi` / Ticket / workflow / docs / memory edits, requires validation/status reporting, and forbids merge/push/close/delete.
- Reviewer guidance states sibling/read-only-by-default review, diff/validation-based judgement, blocker classification, branch-local verdict capture, and no final main-branch approval/merge/close/push.
- Panel Queue notification guidance preserves the queue/inprogress boundary and only mentions `.worktree/<task-name>`, `.yoi` exclusion, sibling coder/reviewer Pods, and merge-ready dossier after inprogress acceptance.
- No broad scheduler or worktree manager was added; the diff is prompt/notification guidance plus focused tests.
- Tests cover the prompt/notification contract for this slice.

Reviewer validation:
- `cargo test -p client ticket_role -- --nocapture`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract -- --nocapture`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `nix build .#yoi --no-link`
- child-built `yoi ticket doctor` against main workspace
- `git merge-tree --write-tree develop HEAD`

Merge readiness: approved; implementation worktree clean and merge-tree against current `develop` succeeded.

---

<!-- event: state_changed author: hare at: 2026-06-07T05:55:57Z from: ready to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T05:55:57Z status: closed -->

## Closed

Implemented, reviewed, merged, and validated.

Summary:
- Added role-specific Orchestrator/Coder/Reviewer guidance for accepted in-progress Tickets.
- Orchestrator guidance now routes only after `workflow_state = inprogress`, references `worktree-workflow` and `multi-agent-workflow`, records bounded progress, and stops at a merge-ready dossier.
- Coder guidance keeps implementation in the child worktree/branch, forbids main `.yoi` / Ticket / workflow / docs / memory edits, requires validation/reporting, and forbids merge/close/delete/push.
- Reviewer guidance keeps review sibling/read-only by default, focused on diff/intent/validation/blocker classification, with branch-local verdicts held for the merge-ready dossier rather than final main Ticket approval.
- Panel Queue notification guidance now describes the post-acceptance worktree/coder/reviewer sibling route and merge-ready dossier stop without implementing merge/close.
- No broad scheduler or worktree manager was added.

Implementation:
- Child commit: `c7d6bb8 orchestrator: add agent routing guidance`
- Merge commit: `merge: orchestrator agent routing`

Review:
- External reviewer `orchestrator-agent-routing-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`

---
