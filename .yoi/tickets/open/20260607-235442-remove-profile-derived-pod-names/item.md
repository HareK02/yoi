---
id: '20260607-235442-remove-profile-derived-pod-names'
slug: 'remove-profile-derived-pod-names'
title: 'Remove Profile-derived Pod names'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['profile', 'pod', 'identity', 'manifest', 'bug']
workflow_state: 'inprogress'
created_at: '2026-06-07T23:54:42Z'
updated_at: '2026-06-08T01:53:53Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T00:32:56Z'
---

## Background

Profiles are intended to be reusable role/config recipes: model, worker behavior, tools, scope intent, memory/web/compaction, prompts/skills, etc. Pod identity is runtime state and should not be authored by a Profile.

The current Profile resolver still has a fallback that derives `pod.name` from the Profile `slug` or source name when no runtime Pod name override is supplied:

```rust
let pod_name = options
    .pod_name
    .unwrap_or_else(|| derive_pod_name(&source, profile.slug.as_deref()));
```

This caused a real bug in the workspace panel: the panel intended to spawn the workspace Companion as `yoi`, but because no explicit Pod name was passed to the child profile startup path, the project default profile `project:companion` resolved to Pod name `companion` from its slug.

The Lua Profile surface should not be able to define or imply Pod identity. Even if `pod.name` is not directly accepted inside the Lua Profile schema, `slug -> pod.name` fallback is still a profile-derived identity path and should be removed.

## Goal

Remove Profile-derived Pod naming. Pod name must come from runtime identity selection, not from Lua Profile fields, Profile slug, Profile source name, or registry entry name.

## Requirements

- Remove `profile.slug` / Profile source / registry entry fallback as a `pod.name` derivation path.
- Profile resolution should require an explicit runtime Pod name input, or a caller-provided default Pod name policy outside Profile resolution.
- Lua Profiles must remain reusable recipes and must not define Pod identity.
- Keep `slug` only as profile metadata/selection identity if still useful; it must not become `pod.name`.
- Reject any direct Lua Profile `pod.name` / manifest-shaped identity field as today, and update diagnostics/tests to make the boundary clear.
- Update callers that currently rely on implicit Profile-derived names to supply a runtime Pod name explicitly.
- Ensure workspace project profiles such as `project:companion`, `project:coder`, etc. do not create Pods named `companion` / `coder` merely because of their slug.
- Revisit or remove `--profile-pod-name` if it exists only to compensate for Profile-derived naming. Prefer a clearer API where `--pod <name>` is the Pod identity and `--profile <selector>` is the recipe selection, if compatible with the CLI model.
- Add tests with non-`yoi` workspace names so dogfooding does not mask profile-derived naming bugs.

## Acceptance criteria

- Resolving `project:companion` without a runtime Pod name no longer yields `pod.name = companion`.
- All fresh Pod creation paths supply Pod identity from runtime policy, not Profile slug/source.
- Existing `SpawnPod` / Ticket role / Panel lifecycle paths still pass explicit names and continue working.
- Profile metadata still records profile slug/source for diagnostics, but does not affect Pod identity.
- Tests cover project profile slug not becoming Pod name.
- `cargo test -p manifest profile --lib`, relevant TUI/client/pod tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
