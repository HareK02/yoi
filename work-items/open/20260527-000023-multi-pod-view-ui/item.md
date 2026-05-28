---
id: 20260527-000023-multi-pod-view-ui
slug: multi-pod-view-ui
title: 複数のPodのViewを行き来できるUI
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:23Z
updated_at: 2026-05-28T14:16:02Z
assignee: null
legacy_ticket: null
---

## Migration reference

- legacy_ticket: null
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# 複数のPodのViewを行き来できるUI

## Background

This work item was migrated from an unfinished TODO.md entry that did not have a dedicated legacy ticket file.

This ticket is intentionally downstream of the shared TUI Pod list/view abstraction. The concrete multi-Pod view requirements should be defined after the common list/view model exists, so this ticket can focus on view switching and interaction policy rather than inventing another Pod list representation.

## Prerequisite

- `20260528-141602-tui-pod-list-view-abstraction`

## Acceptance criteria

- Define the concrete requirements before implementation.
