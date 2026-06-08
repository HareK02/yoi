---
id: '20260607-235505-use-workspace-basename-for-default-pod-name'
slug: 'use-workspace-basename-for-default-pod-name'
title: 'Use workspace basename for default Pod name'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['pod', 'identity', 'workspace', 'cli', 'panel']
workflow_state: 'inprogress'
created_at: '2026-06-07T23:55:05Z'
updated_at: '2026-06-08T00:57:20Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T00:33:01Z'
---

## Background

Dogfooding happens in a repository whose directory name is `yoi`, which masked hardcoded/default Pod naming problems. Some runtime paths still use `DEFAULT_POD_NAME = "yoi"` or otherwise derive a default identity that is product-name-specific rather than workspace-specific.

At the same time, other paths already default Pod names from the current workspace directory basename. The intended behavior is to unify on workspace-based runtime naming:

- normal workspace Companion/default Pod name is the workspace directory basename;
- this repository's name happens to be `yoi`, but that should be an outcome of basename derivation, not a hardcoded default;
- Orchestrator and role/task Pods can derive distinct names from the workspace/task/role, but should be explicit runtime names;
- Profile selection must not determine Pod identity.

## Goal

Unify default Pod naming around the workspace/current-directory basename and remove product-name-specific hardcoded defaults such as `"yoi"` from runtime identity selection.

## Requirements

- Audit Pod naming paths and identify every use of hardcoded `"yoi"` / `DEFAULT_POD_NAME` as runtime identity.
- Replace default fresh Pod naming with sanitized workspace/current-directory basename where appropriate.
- Keep a generic fallback such as `"pod"` only when no useful workspace basename is available.
- Ensure top-level fresh startup, TUI spawn form defaults, Panel Companion lifecycle, and CLI/runtime helper paths use consistent naming policy.
- Preserve explicit `--pod <name>` behavior: explicit names always win.
- Keep workspace Orchestrator distinct from Companion/default Pod, e.g. `<workspace>-orchestrator`.
- Ensure Ticket role Pods continue to use explicit role/task-derived names and do not fall back to the generic workspace Companion name unless intended.
- Add tests using a non-`yoi` workspace directory to prove the default Pod name is the directory basename.
- Coordinate with `remove-profile-derived-pod-names`: Profile slug/source must not influence the default Pod name.
- Update docs/help text if they imply the default Pod name is `yoi`.

## Non-goals

- Do not rename historical existing Pod metadata automatically.
- Do not change explicit Pod names already stored in existing metadata.
- Do not implement archive/fresh-start behavior here; that is covered by `add-pod-archive-fresh-start-path`.
- Do not redesign product/repository naming beyond removing hardcoded runtime identity defaults.

## Acceptance criteria

- In a workspace directory named `example-project`, fresh default startup creates/uses Pod name `example-project` unless an explicit name is provided.
- In this repository, the default remains `yoi` only because the directory basename is `yoi`.
- No runtime fresh-start path relies on `DEFAULT_POD_NAME = "yoi"` as the normal identity default.
- Panel Companion missing spawn requests and verifies the workspace basename Pod name.
- Orchestrator remains `<workspace>-orchestrator` or equivalent distinct runtime name.
- Tests cover non-`yoi` workspace naming and explicit `--pod` precedence.
- Relevant `cargo test` suites, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
