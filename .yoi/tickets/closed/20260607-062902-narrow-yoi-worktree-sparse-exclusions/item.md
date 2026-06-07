---
id: 20260607-062902-narrow-yoi-worktree-sparse-exclusions
slug: narrow-yoi-worktree-sparse-exclusions
title: Narrow child worktree .yoi sparse exclusions
status: closed
kind: task
priority: P1
labels: [worktree, workflow, memory, ticket, orchestration]
workflow_state: done
created_at: 2026-06-07T06:29:02Z
updated_at: 2026-06-07T07:38:29Z
assignee: null
legacy_ticket: null
---

## Background

Current worktree workflow excludes `.yoi/**` from child implementation worktrees. This was originally a conservative way to keep generated/personal memory and runtime state out of child worktrees. As Ticket/Workflow orchestration has become project-record based, excluding all of `.yoi` is too broad.

Project records such as `.yoi/tickets`, `.yoi/workflow`, `.yoi/knowledge`, and `.yoi/ticket.config.toml` may need to be visible in child worktrees for branch-local artifacts, workflow edits, review dossiers, and Ticket-aware implementation. Generated memory/runtime/local files should still be excluded.

## Goal

Narrow child worktree sparse-checkout exclusions so git-tracked Yoi project records can appear in child worktrees while memory/local/runtime state remains excluded.

## Requirements

- Update `worktree-workflow` and any worktree creation helpers/scripts to stop excluding all of `.yoi/**`.
- Include git-tracked project record paths where appropriate:
  - `.yoi/tickets/**`;
  - `.yoi/workflow/**`;
  - `.yoi/knowledge/**` when curated/tracked;
  - `.yoi/ticket.config.toml`;
  - other tracked Yoi project config/resources as needed.
- Continue excluding generated/personal/local/runtime paths, including at least:
  - `.yoi/memory/**`;
  - `.yoi/**/_logs/**`;
  - `.yoi/tickets/.ticket-backend.lock`;
  - local/override/secret-like files according to existing ignore/config policy.
- Define the edit policy for Ticket records in child worktrees:
  - branch-local artifacts / merge-ready dossier may be written in child worktree when part of the implementation branch;
  - active orchestration progress and final Ticket review/approval/close remain main-workspace responsibilities unless explicitly designed otherwise;
  - avoid concurrent edits to the same Ticket thread from main and child worktree.
- Ensure generated/personal memory is not copied into child worktrees.
- Update tests/docs/workflows that currently assume `.yoi` is absent from child worktrees.

## Non-goals

- Moving memory storage out of `.yoi/memory`; that belongs to the memory root marker ticket.
- Making child implementation Pods responsible for final Ticket closure.
- Allowing secret/local files into child worktrees.
- Replacing the main workspace as the orchestration authority.

## Acceptance criteria

- New child worktrees include relevant tracked `.yoi` project records but exclude `.yoi/memory` and local/runtime files.
- Worktree workflow documentation states the new sparse-checkout rules and Ticket edit policy.
- Child worktree creation validation checks that memory/local paths are absent while project record paths can be present.
- Orchestrator workflows can use child worktrees for branch-local artifacts/dossiers without copying generated memory.
