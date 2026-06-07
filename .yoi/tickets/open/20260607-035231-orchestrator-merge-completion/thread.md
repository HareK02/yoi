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
