---
description: Public sibling coder/reviewer workflow
model_invokation: false
user_invocable: true
requires: [workflow-resource-boundary]
---
# Multi-agent Workflow

Use sibling implementation and review roles for a bounded Ticket. The Orchestrator owns intent, acceptance boundaries, blocker decisions, final merge-completion authority, and cleanup. The coder implements within delegated scope; the reviewer checks the recorded Ticket intent and acceptance criteria rather than unrecorded preferences.

Produce a merge-ready dossier with Ticket id, branch/worktree, commits, implementation summary, reviewer verdict, validation evidence, residual risks, dirty state, and any remaining human decision needs.
