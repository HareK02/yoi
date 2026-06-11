---
title: "Builtin Workflow and Knowledge resources"
state: 'ready'
created_at: "2026-06-07T07:27:08Z"
updated_at: '2026-06-11T07:56:31Z'
---

## Background

Recent Orchestrator automation work embedded key workflow contracts as role-specific builtin guidance in generated prompts. This is sufficient for now, but it is not a full builtin Workflow/Knowledge resource system.

Longer term, reusable Yoi workflows and curated guidance should be available as bundled builtin Workflow/Knowledge resources that can be resolved by slug and optionally overridden/extended by project/user records.

## Goal

Add builtin Workflow/Knowledge resources as first-class bundled resources while preserving project/user override capability.

## Requirements

- Define a bundled resource location for builtin Workflows and builtin Knowledge, e.g. under `resources/`.
- Add resolver support for builtin Workflow/Knowledge slugs such as `builtin:multi-agent-workflow` or equivalent source-qualified identifiers.
- Preserve project-authored records under `.yoi/workflow` and `.yoi/knowledge` as project overrides/extensions.
- Define precedence between builtin, user, and project resources.
- Make builtin resource resolution available to role/prompt generation and workflow invocation paths where appropriate.
- Keep existing role-specific prompt guidance working; do not require a broad migration in the first pass.
- Ensure builtin resources can be referenced without copying them into every workspace.
- Document how users/projects override or extend builtin Workflow/Knowledge.
- Add tests for resource lookup, override precedence, and missing-resource diagnostics.

## Non-goals

- Rewriting all current workflow prompts immediately.
- Solving active workflow compaction persistence; that remains `preserve-active-workflows-across-compaction`.
- Removing project `.yoi/workflow` / `.yoi/knowledge` support.

## Acceptance criteria

- Builtin Workflow/Knowledge resources can be resolved by explicit source-qualified slug.
- Project/user resources can override or extend builtin resources according to documented precedence.
- Existing project workflow files continue to work.
- Tests cover builtin lookup and override behavior.
