# Workflows and orchestration

Yoi development uses workflows to make multi-step agent work repeatable without hiding authority in chat state.

Project-authored workflows live under `.yoi/workflow/`. Generated memory lives under `.yoi/memory/`; the two should not be mixed.

## Workflow role

A workflow should define how to coordinate work. It should not become a private implementation branch, an unreviewed design decision, or a replacement for Tickets.

Current workflow themes include:

- Intake clarification before materializing user requests as Tickets
- Orchestrator routing from Tickets to the next workflow/action
- planning/requirements synchronization when concrete missing decisions or information block routing
- worktree setup and cleanup
- sibling coder/reviewer Pod orchestration
- human-gated maintenance and merge readiness

## Child Pods

Spawned Pods are useful for scoped implementation, review, or exploration. They are not independent project authorities.

A parent/orchestrator must verify:

- child output via `ReadPodOutput`
- live/restorable state via Pod tools when relevant
- worktree state and diff
- validation command output
- Ticket requirements and acceptance criteria

Notifications are hints to inspect state. They are not proof of completion.

## Merge and close responsibility

Unless explicitly authorized otherwise, final merge, cleanup, design-boundary decisions, and ticket closure remain the orchestrator/human responsibility.

Child Pods may commit in delegated worktrees when the workflow allows it, but the merge-ready dossier should make the final decision auditable from repository records.

## Public and dogfood workflow split

Public builtin workflow candidates and this repository's dogfood-specific workflow mechanics are tracked in `docs/design/workflows-public-dogfood-split.md`.
