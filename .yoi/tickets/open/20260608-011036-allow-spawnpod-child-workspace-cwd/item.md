---
id: '20260608-011036-allow-spawnpod-child-workspace-cwd'
slug: 'allow-spawnpod-child-workspace-cwd'
title: 'Allow SpawnPod to specify child workspace cwd'
status: 'open'
kind: 'task'
priority: 'P2'
labels: ['pod', 'spawn', 'workspace', 'worktree', 'orchestration']
workflow_state: 'intake'
created_at: '2026-06-08T01:10:36Z'
updated_at: '2026-06-08T01:10:36Z'
assignee: null
legacy_ticket: null
---

## Background

Current multi-agent/worktree workflows assume Coder/Reviewer Pods are spawned with the parent Orchestrator's cwd, usually the main workspace. Worktree isolation is enforced by delegated write scope and by task text instructing the Coder to run every Bash command with `cd <repo>/.worktree/<task-name> && ...`.

Current workflow text explicitly says:

```text
Pod の cwd は main workspace のままになる。必ず作業対象が child worktree であることを明示し、Bash 実行時は毎回 cd <repo>/.worktree/<task-name> && ... させる。
```

This works but is fragile and awkward:

- Coder can forget to `cd` before commands.
- Tool cwd, profile/project discovery, AGENTS.md, and runtime workspace identity remain main-workspace biased.
- Command snippets are noisy and error-prone.
- Child worktree behavior is controlled by prompt discipline rather than runtime configuration.

`SpawnPod` currently has no LLM-facing cwd/workspace field; internally child processes are launched with `Command::current_dir(self.spawner_pwd)`.

## Goal

Allow `SpawnPod` callers to specify the child Pod's runtime workspace/cwd, so implementation Pods can be started directly inside their child worktree while keeping explicit scope delegation and orchestration authority boundaries.

## Requirements

- Add a `SpawnPod` input field for child runtime workspace/cwd.
  - Candidate names: `workspace`, `workspace_root`, or `cwd`.
  - Prefer terminology aligned with `introduce-runtime-workspace-root-context` if that lands first.
- When omitted, preserve current behavior: child cwd/workspace defaults to the spawner Pod's pwd.
- When provided:
  - Validate the path is absolute.
  - Validate the path exists and is a directory.
  - Validate the parent has at least read authority for the path.
  - Set the child process `current_dir` to that path.
  - Pass the runtime workspace context to the child Pod/profile resolver consistently if such context exists.
- Ensure delegated scope remains explicit and independent from cwd.
  - cwd does not grant permission by itself.
  - Child write scope must still be supplied through `scope` and validated against parent/delegation authority.
- Support the standard worktree case:
  - child cwd/workspace = `<repo>/.worktree/<task-name>`;
  - child read scope may include main repo if needed;
  - child write scope is the child worktree or narrower.
- Update `worktree-workflow` and `multi-agent-workflow` to prefer spawning Coder Pods with cwd/workspace set to the child worktree instead of telling them to `cd` every time.
- Keep main workspace authority responsibilities explicit:
  - ticket orchestration progress, final review/approval/close, and merge authority remain with Orchestrator/main workspace unless intentionally delegated.
- Ensure child worktree `.yoi` sparse rules and `.yoi/memory` behavior remain intact.
- Add diagnostics if requested cwd/workspace is not permitted or invalid.

## Design questions

- Should the field be named `cwd` for OS process current directory, or `workspace_root` to align with runtime identity/profile discovery?
- Should `workspace_root` affect default Pod name if `name` is explicitly required by `SpawnPod`?
- How should profile discovery behave when child cwd is a worktree containing tracked `.yoi` project records but not `.yoi/memory`?
- Should reviewer Pods also use child worktree cwd by default, or keep main workspace cwd for comparing main vs branch? The workflow should state the preferred pattern.

## Acceptance criteria

- `SpawnPod` can launch a child Pod with process cwd set to an explicit child worktree path.
- The child Pod's Bash tool starts fresh shells rooted at that child worktree without requiring `cd` in every command.
- Omitting the field preserves current behavior.
- Invalid/non-directory/out-of-scope cwd requests fail with clear errors.
- cwd/workspace selection does not grant implicit read/write permission.
- Worktree/multi-agent workflows are updated to use the new field for coder Pods.
- Tests cover omitted/default cwd, explicit valid cwd, invalid cwd, and permission denial.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
