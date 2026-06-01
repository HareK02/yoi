# Workflows and orchestration

Yoi development uses workflows to make multi-step agent work repeatable without hiding authority in chat state.

Project-authored workflows live under `.yoi/workflow/`. Generated memory lives under `.yoi/memory/`; the two should not be mixed.

## Workflow role

A workflow should define how to coordinate work. It should not become a private implementation branch, an unreviewed design decision, or a replacement for work items.

Current workflow themes include:

- preflight before delegating uncertain ticket work
- worktree setup and cleanup
- sibling coder/reviewer Pod orchestration
- human-gated maintenance and merge readiness

## Child Pods

Spawned Pods are useful for scoped implementation, review, or exploration. They are not independent project authorities.

A parent/orchestrator must verify:

- child output via `ReadPodOutput`
- live/restorable state via Pod tools when relevant
- worktree status and diff
- validation command output
- work item requirements and acceptance criteria

Notifications are hints to inspect state. They are not proof of completion.

## Merge and close responsibility

Unless explicitly authorized otherwise, final merge, cleanup, design-boundary decisions, and ticket closure remain the orchestrator/human responsibility.

Child Pods may commit in delegated worktrees when the workflow allows it, but the merge-ready dossier should make the final decision auditable from repository records.
