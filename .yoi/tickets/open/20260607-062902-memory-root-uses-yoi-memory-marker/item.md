---
id: 20260607-062902-memory-root-uses-yoi-memory-marker
slug: memory-root-uses-yoi-memory-marker
title: Use .yoi/memory marker for repo-local memory root
status: open
kind: task
priority: P1
labels: [memory, workspace, worktree, config]
workflow_state: ready
created_at: 2026-06-07T06:29:02Z
updated_at: 2026-06-07T07:53:06Z
assignee: null
legacy_ticket: null
---

## Background

`.yoi/` currently acts as a broad Yoi workspace/project marker, but using `.yoi` existence as the memory workspace/root marker is too coarse. `.yoi` now contains project records such as Tickets, workflows, Knowledge, and config, while `.yoi/memory` is generated/personal memory state.

Child worktrees should be able to include `.yoi/tickets` and `.yoi/workflow` without becoming independent memory roots. Memory root detection should key off `.yoi/memory` or explicit memory configuration rather than `.yoi` alone.

## Goal

Separate Yoi project/workspace detection from repo-local memory root detection by treating `.yoi` as the project records marker and `.yoi/memory` as the repo-local memory root marker.

## Target model

- `.yoi/` means: this repository/workspace has Yoi project records/config.
- `.yoi/memory/` means: this workspace or ancestor is the repo-local memory root.
- Child worktrees may contain `.yoi/tickets` / `.yoi/workflow` without causing memory writes into the child worktree.
- Memory root lookup can walk ancestors until it finds `.yoi/memory` or an explicit configured root.

## Requirements

- Audit current workspace/memory root detection code that uses `.yoi` existence.
- Change memory root detection so `.yoi` alone is not enough to select a memory root.
- Use an explicit memory root if configured.
- If repo-local memory is enabled and no explicit root is configured, use nearest ancestor containing `.yoi/memory`.
- If memory is explicitly enabled for a workspace and no `.yoi/memory` exists yet, create the configured/default memory directory lazily rather than requiring both config and preexisting directory.
- If memory is explicitly disabled, do not use `.yoi/memory` even if present, unless a separate compatibility mode intentionally says otherwise.
- Decide and document fallback behavior when no explicit memory config and no ancestor `.yoi/memory` exists:
  - disabled; or
  - user-data workspace overlay;
  - but do not silently treat `.yoi` alone as memory root.
- Preserve existing generated-memory safety rules: do not copy memory into child worktrees and keep memory logs/generated records out of git-tracked project artifacts.
- Update docs/tests for child worktree behavior and memory root lookup.

## Non-goals

- Removing `.yoi` as a Yoi project records marker.
- Removing repo-local `.yoi/memory` support entirely.
- Changing Ticket/workflow/project record roots.
- Implementing the child worktree sparse-checkout change; that belongs to `narrow-yoi-worktree-sparse-exclusions`.

## Acceptance criteria

- `.yoi` alone no longer selects a memory root.
- `.yoi/memory` can be found by ancestor walk and used as repo-local memory root when enabled/allowed.
- A child worktree containing `.yoi/tickets` but not `.yoi/memory` does not become an independent memory workspace.
- Explicit memory enable/disable behavior is tested.
- Documentation clearly distinguishes `.yoi` project records marker from `.yoi/memory` repo-local memory marker.
