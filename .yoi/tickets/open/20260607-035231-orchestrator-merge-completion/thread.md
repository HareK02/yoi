<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:52:31Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-07T05:00:22Z -->

## Decision

## Git/Ticket review record boundary

Decision:
- Reviewer verdicts on an unmerged implementation branch are branch-scoped evidence, not final main-branch Ticket approval.
- The Orchestrator should keep branch-local review results in the merge-ready dossier/review report during the worktree-agent phase.
- Main Ticket thread may still record durable progress such as worktree plan, coder/reviewer delegation, blockers, and fix-loop status.
- The final Ticket review/approval/completion record should be written during the merge-completion phase, after confirming the reviewed branch is the branch being merged, or as part of the same controlled completion sequence.

Rationale:
- Avoid main branch Ticket history claiming approval for code that is not yet in main.
- Keep git history aligned: implementation branch review evidence leads to merge; main Ticket lifecycle records the completed merge/validation outcome.
- Builtin Orchestrator workflow should make this explicit so automation does not reproduce the ad-hoc parent-side `TicketReview` timing used during manual dogfooding.

---

<!-- event: plan author: hare at: 2026-06-07T06:15:24Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready as the third Orchestrator automation slice, after queued routing and worktree-agent routing landed.

Intent:
- Add explicit Orchestrator merge/completion guidance or focused wiring for reviewed in-progress Tickets that have a merge-ready dossier.
- Preserve the git/Ticket boundary: branch-local reviewer verdicts are evidence in the dossier before merge; final main Ticket approval/completion records are written only during the controlled merge-completion phase after confirming the reviewed branch is the branch being merged.
- Support dogfooding authorization for Orchestrator merge/cleanup/close, while conservative/missing authorization stops at merge-ready dossier.

Prerequisite state:
- `orchestrator-queued-ticket-routing` landed: Queue notification authorizes routing and requires `queued -> inprogress` before side effects.
- `orchestrator-worktree-agent-routing` landed: accepted Tickets can be routed through worktree + coder/reviewer sibling guidance and stop at merge-ready dossier.
- Ticket lifecycle tools and role launch config strictness/scaffold have landed.

Requirements for this slice:
- Add merge-completion guidance to the relevant Orchestrator role prompt/workflow surface.
- Merge conditions:
  - Ticket is `inprogress` and has a merge-ready dossier;
  - reviewed branch/worktree/commits in the dossier match the branch to be merged;
  - independent reviewer approval exists in the dossier, or a human override decision is explicitly recorded;
  - main workspace is safe and unrelated dirty changes are understood;
  - merge authority is available in dogfooding/workspace policy, otherwise stop at dossier.
- Merge/completion sequence:
  - stop/reclaim coder/reviewer Pods where appropriate;
  - `git merge --no-ff <branch>` or project-agreed merge method;
  - run post-merge validation appropriate to the change;
  - record review/merge/validation outcome in the Ticket thread during merge-completion, not before;
  - transition `inprogress -> done` or close the Ticket according to typed Ticket workflow rules;
  - remove merged child worktree and delete merged branch unless explicitly kept.
- Add/update tests for merge authority boundary and required dossier/validation fields in generated guidance or helper text.

Likely implementation surfaces:
- `crates/client/src/ticket_role.rs`: existing Orchestrator role guidance already stops at merge-ready dossier; extend with merge-completion guidance for the authorized completion phase.
- `crates/tui/src/multi_pod.rs`: Queue notification should probably remain focused on routing/agent path and not over-expand into merge details unless existing tests require consistency.
- `.yoi/workflow/multi-agent-workflow.md` may need parent-side wording updates to align with branch-local verdict vs main Ticket approval boundary. Child worktree excludes `.yoi`, so any workflow edits must be reported for parent-side application.

Non-goals:
- Do not implement Queue notification/acceptance.
- Do not implement worktree/coder/reviewer routing.
- Do not remove human override path.
- Do not make merge authority implicit for all public/default configurations.
- Do not solve active workflow compaction persistence here.

Validation:
- Run focused tests for changed crates, especially `cargo test -p client ticket_role --lib` if role guidance changes.
- Run `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` if Ticket/workflow records are touched.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T06:15:32Z -->

## Intake summary

Implementation-ready as the third Orchestrator automation slice: add merge-completion guidance for reviewed in-progress Tickets with merge-ready dossiers, preserving branch-local review vs final main Ticket approval boundaries, dogfooding merge authorization, post-merge validation, Ticket completion, and worktree/branch/Pod cleanup.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T06:15:32Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T06:16:17Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `orchestrator-merge-coder-20260607` in worktree `.worktree/orchestrator-merge-completion` on branch `work/orchestrator-merge-completion`.

Scope:
- Add Orchestrator merge/completion guidance for reviewed in-progress Tickets with merge-ready dossiers.
- Preserve branch-local reviewer verdict vs final main Ticket approval boundary.
- Define dogfooding merge authority vs conservative stop-at-dossier behavior.
- Include post-merge validation, Ticket completion, worktree/branch/Pod cleanup guidance.
- Do not implement queue routing or worktree/coder/reviewer routing.

Workflow note:
- Child worktree excludes `.yoi`; if workflow file updates are needed, the child should report an exact parent-side patch rather than editing `.yoi`.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, workflow file updates, and cleanup remain with the parent/human workflow.

---
