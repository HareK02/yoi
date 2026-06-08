---
id: '20260608-000047-introduce-runtime-workspace-root-context'
slug: 'introduce-runtime-workspace-root-context'
title: 'Introduce runtime workspace root context'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['workspace', 'pod', 'profile', 'cli', 'panel', 'memory']
workflow_state: 'inprogress'
created_at: '2026-06-08T00:00:47Z'
updated_at: '2026-06-08T01:49:15Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T00:14:09Z'
---

## Background

Workspace/root concepts are currently inferred in multiple places from `current_dir()`, `.yoi` project records, `.yoi/memory`, profile resolution, Panel lifecycle, and Pod runtime startup. This has caused repeated boundary confusion:

- profile slug/source can leak into Pod identity;
- hardcoded `DEFAULT_POD_NAME = "yoi"` is masked by dogfooding in a repo named `yoi`;
- Panel Companion intended to spawn workspace basename `yoi`, but a profile-derived name `companion` was created;
- memory root detection was already corrected to use explicit `memory.workspace_root` or nearest `.yoi/memory`, not `.yoi` alone;
- child worktrees may contain `.yoi` project records but should not inherit memory root or Pod identity from the parent unless explicitly intended.

The system needs a single runtime workspace context decided at process/Pod startup and passed through the relevant resolution layers, rather than rediscovered ad hoc at each use site.

## Goal

Introduce an explicit runtime `workspace_root` launch context that is determined once at startup (defaulting to process cwd or an explicit CLI argument) and then passed to profile resolution, Pod naming, Panel lifecycle, Ticket/project record discovery, and memory resolution as appropriate.

## Requirements

### Runtime workspace context

- Add a runtime launch context field such as `workspace_root: PathBuf` to the relevant startup/config structures.
- Determine it at top-level startup:
  - default to process cwd;
  - support an explicit CLI option such as `--workspace <path>` if appropriate for both normal TUI/Pod and panel flows.
- Pass the resolved workspace root explicitly to child/runtime Pod startup instead of relying on child process cwd where possible.
- Persist/snapshot the resolved workspace root in Pod metadata/session state where needed for restore/debug visibility.

### Boundary rules

- `workspace_root` is runtime/user intent, not Profile-authored config.
- Profile selection must not determine `workspace_root` or `pod.name`.
- Default Pod naming should derive from `workspace_root.file_name()` via the separate Pod naming cleanup ticket.
- Project records root (`.yoi/tickets`, `.yoi/workflow`, `.yoi/profiles.toml`) may be discovered from or relative to the runtime workspace, but must remain conceptually separate from memory root.
- Memory root behavior remains:
  - explicit `memory.workspace_root` wins;
  - otherwise nearest ancestor with `.yoi/memory` from the runtime workspace/default root;
  - `.yoi` alone is not a memory root marker.
- Child worktrees should use their own runtime workspace root when launched there, even if memory/project records resolve to an ancestor.

### CLI / process boundaries

- Prefer a clear boundary:
  - `--workspace <path>` = runtime workspace root;
  - `--pod <name>` = Pod identity;
  - `--profile <selector>` = Profile recipe.
- Avoid `--profile-pod-name` as a long-term public surface; if still needed internally, document and plan removal/renaming.
- Ensure Panel, Ticket role launcher, SpawnPod/client spawn helpers, and normal TUI startup pass the same workspace context consistently.

### Tests / validation

- Add tests using non-`yoi` workspace directory names to avoid dogfooding masking.
- Add tests for child worktree cases where `.yoi` project records are present but `.yoi/memory` is absent.
- Verify runtime workspace basename, project profile discovery, and memory root resolution do not collapse into the same concept.

## Related tickets

- `remove-profile-derived-pod-names`
- `use-workspace-basename-for-default-pod-name`
- `add-pod-archive-fresh-start-path`
- `memory-root-uses-yoi-memory-marker` is already implemented and should be preserved.

## Non-goals

- Do not redesign memory root resolution beyond preserving the existing `.yoi/memory` marker behavior.
- Do not rename existing Pod metadata automatically.
- Do not implement profile-derived naming removal or default Pod naming cleanup here if those are handled by the related tickets; coordinate boundaries.

## Acceptance criteria

- There is a single explicit runtime workspace root available to startup, profile resolution, Panel lifecycle, and Pod runtime code.
- Normal startup and `yoi panel` can be pointed at a workspace with `--workspace` or an equivalent explicit context.
- Default Pod naming and Panel Companion naming can consume this runtime workspace root without re-detecting via `.yoi` or memory root.
- Project records and memory root resolution remain separate and tested.
- Non-`yoi` workspace tests prove runtime workspace identity does not fall back to hardcoded product names.
- Relevant docs/help text are updated.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
