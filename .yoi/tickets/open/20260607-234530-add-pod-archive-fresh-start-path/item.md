---
id: 20260607-234530-add-pod-archive-fresh-start-path
slug: add-pod-archive-fresh-start-path
title: Add Pod archive and fresh-start path
status: open
kind: task
priority: P2
labels: [pod, panel, cli, lifecycle, metadata]
workflow_state: 'planning'
created_at: 2026-06-07T23:45:30Z
updated_at: '2026-06-08T03:49:14Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T03:48:46Z'
---

## Background

Panel Companion and Orchestrator lifecycle currently restore restorable Pods by name before spawning a missing Pod. This is correct for normal continuity, but after updating workflows/profiles/system guidance, a user may want to intentionally start a fresh same-name Pod instead of restoring old Pod metadata/history.

Currently there is no explicit Panel/CLI path to archive or reset a restorable Pod name before fresh creation. The practical workaround is to manually rename/archive the Pod metadata directory under the user data dir, e.g. `~/.yoi/pods/<pod-name>/`, which is unsafe and not discoverable.

Example:

- Workspace Companion name is `yoi`.
- `yoi panel` sees `yoi` as restorable and restores it.
- User wants a fresh `yoi` Companion after workflow/profile changes.
- There is no `fresh` / `archive` action in Panel or CLI, so manual metadata directory surgery is the only apparent route.

## Goal

Provide an explicit, safe Pod archive/reset/fresh-start path for same-name Pods so users do not need to manually edit `~/.yoi/pods` metadata.

## Requirements

- Add a user-facing way to archive or reset a stopped/restorable Pod name so the next same-name launch is fresh.
- Refuse to archive/reset a live reachable Pod unless it is explicitly stopped first.
- Preserve old metadata/history by default; prefer archive/rename over destructive delete.
- Make the archive location/name deterministic and collision-safe.
- Surface what will happen clearly:
  - archived Pod metadata will no longer be restored by name;
  - session logs/history are preserved where applicable;
  - next launch with the same name will create a fresh Pod.
- Support the Panel Companion use case where the workspace Companion name is fixed to the workspace basename, e.g. `yoi`.
- Consider both CLI and Panel/TUI surfaces.
  - CLI examples could be `yoi pod archive <name>` or `yoi pod reset <name>`.
  - Panel could expose a bounded action/diagnostic path for `fresh Companion` if appropriate.
- Keep normal lifecycle behavior unchanged: live -> use, restorable -> restore, missing -> spawn.
- Do not silently bypass restore just because profiles/workflows changed; fresh must be explicit.
- Avoid manual modification of `~/.yoi/pods` in docs except as an emergency/debug note.

## Acceptance criteria

- A stopped/restorable Pod can be archived via an official command/action.
- After archive, the same Pod name is treated as missing for attach/panel lifecycle and can be spawned fresh.
- Live Pods are protected from archive/reset without an explicit stop path.
- Archive preserves enough metadata/history for later inspection or manual recovery.
- Panel Companion `yoi` can be refreshed without hand-renaming data-dir metadata.
- Tests cover archive refusal for live Pods, archive success for stopped/restorable Pods, and fresh same-name spawn after archive.
- Relevant docs/help text are updated.
- `cargo test` focused suites, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
