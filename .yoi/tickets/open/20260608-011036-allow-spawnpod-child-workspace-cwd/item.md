---
id: '20260608-011036-allow-spawnpod-child-workspace-cwd'
slug: 'allow-spawnpod-child-workspace-cwd'
title: 'Allow SpawnPod to specify child cwd'
status: 'open'
kind: 'task'
priority: 'P2'
labels: ['pod', 'spawn', 'cwd', 'worktree', 'orchestration']
workflow_state: 'inprogress'
created_at: '2026-06-08T01:10:36Z'
updated_at: '2026-06-08T07:07:00Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T03:06:04Z'
---

## Background

Current multi-agent/worktree workflows assume Coder/Reviewer Pods are spawned with the parent Orchestrator's cwd, usually the main workspace. Worktree isolation is enforced by delegated write scope and by task text instructing the Coder to run every Bash command with `cd <repo>/.worktree/<task-name> && ...`.

Current workflow text explicitly says:

```text
Pod の cwd は main workspace のままになる。必ず作業対象が child worktree であることを明示し、Bash 実行時は毎回 cd <repo>/.worktree/<task-name> && ... させる。
```

This works but is fragile and awkward:

- Coder can forget to `cd` before commands.
- Bash/tool execution starts from the main workspace even when all edits should happen in the child worktree.
- Command snippets are noisy and error-prone.
- Child worktree behavior is controlled by prompt discipline rather than runtime configuration.

`SpawnPod` currently has no LLM-facing `cwd` field; internally child processes are launched with `Command::current_dir(self.spawner_pwd)`.

This ticket is **not** about changing the child Pod's runtime workspace root. A spawned child remains in the same project/workspace context as its parent. The improvement is only to let the child process/tool cwd point at a child worktree inside that workspace.

## Goal

Allow `SpawnPod` callers to specify the child Pod's process/tool `cwd`, so implementation Pods can start directly inside their child worktree while inheriting the parent workspace context and keeping explicit scope delegation.

## Requirements

- Add a `SpawnPod` input field named `cwd` for the child process/tool working directory.
- When omitted, preserve current behavior: child cwd defaults to the spawner Pod's pwd.
- `cwd` must not change the runtime workspace root.
  - Child Pods inherit the parent workspace/project context.
  - Profile discovery, project records, Ticket config, workflow registry, memory root resolution start point, and Pod identity policy should continue to use the inherited runtime workspace context unless another explicit design says otherwise.
- When `cwd` is provided:
  - Validate the path is absolute.
  - Validate the path exists and is a directory.
  - Validate the parent has at least read authority for the path.
  - Set the child process `current_dir` to that path.
  - Ensure Bash/tool default cwd uses that path.
- Ensure delegated scope remains explicit and independent from cwd.
  - cwd does not grant permission by itself.
  - Child read/write scope must still be supplied through `scope` and validated against parent/delegation authority.
- Support the standard worktree case:
  - inherited workspace root = main repo;
  - child cwd = `<repo>/.worktree/<task-name>`;
  - child read scope may include main repo if needed;
  - child write scope is the child worktree or narrower.
- Update `worktree-workflow` and `multi-agent-workflow` to prefer spawning Coder Pods with `cwd` set to the child worktree instead of telling them to `cd` every time.
- Keep main workspace authority responsibilities explicit:
  - ticket orchestration progress, final review/approval/close, and merge authority remain with Orchestrator/main workspace unless intentionally delegated.
- Ensure child worktree `.yoi` sparse rules and `.yoi/memory` behavior remain intact.
- Add diagnostics if requested cwd is not permitted or invalid.

## Design notes

- The field should be named `cwd`, not `workspace_root`, because SpawnPod children should not move into a different workspace. They only need a different process/tool working directory inside the same workspace context.
- `cwd` should not affect default Pod name. `SpawnPod.name` is already explicit.
- `cwd` should not affect Profile selection or project profile discovery.
- Reviewer Pods may use main workspace cwd or child worktree cwd depending on review style; workflow guidance should state the recommended default rather than make `cwd` imply review authority.

## Acceptance criteria

- `SpawnPod` can launch a child Pod with process/tool cwd set to an explicit child worktree path.
- The child Pod's Bash tool starts fresh shells rooted at that child worktree without requiring `cd` in every command.
- Omitting `cwd` preserves current behavior.
- Invalid/non-directory/out-of-scope cwd requests fail with clear errors.
- cwd selection does not grant implicit read/write permission and does not alter inherited workspace context.
- Worktree/multi-agent workflows are updated to use the new field for coder Pods.
- Tests cover omitted/default cwd, explicit valid cwd, invalid cwd, permission denial, and workspace-context inheritance.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
