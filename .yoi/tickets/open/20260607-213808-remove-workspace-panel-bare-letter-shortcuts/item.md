---
id: 20260607-213808-remove-workspace-panel-bare-letter-shortcuts
slug: remove-workspace-panel-bare-letter-shortcuts
title: Remove bare letter shortcuts from workspace panel
status: open
kind: task
priority: P2
labels: [tui, panel, ux, keyboard]
workflow_state: intake
created_at: 2026-06-07T21:38:08Z
updated_at: 2026-06-07T21:38:08Z
assignee: null
legacy_ticket: null
---

## Background

The workspace panel keeps the composer visible, but several bare letter keys are currently handled as global panel shortcuts before they can be inserted into the composer. This makes normal text input feel broken: typing `j`, `k`, `o`, or `r` can move selection, open/attach a Pod, or refresh instead of entering text.

Observed shortcuts to remove:

- `j`: next row
- `k`: previous row
- `o`: open / attach / restore selected Pod
- `r`: refresh

The panel should prioritize composer text entry. Navigation/commands should use non-text keys or modifier-based shortcuts.

## Goal

Remove the bare `j` / `k` / `o` / `r` panel shortcuts so those characters are inserted into the composer as ordinary text.

## Requirements

- Remove bare `j` and `k` row-navigation shortcuts from workspace panel key handling.
- Remove bare `o` open/attach shortcut from workspace panel key handling.
- Remove bare `r` refresh shortcut from workspace panel key handling.
- Keep non-text controls such as `↑` / `↓`, `Enter`, `Alt+Enter`, `Ctrl+T`, `Esc`, and `Ctrl+C` unless tests reveal a direct conflict.
- Update actionbar/help text so it no longer advertises `o open` or `r refresh`.
- Ensure `j`, `k`, `o`, and `r` are inserted into the composer for normal input.
- Do not reintroduce selected-Pod direct-send semantics.
- Do not change Ticket action dispatch semantics except where tests must be updated for removed shortcuts.

## Acceptance criteria

- Typing `j`, `k`, `o`, or `r` in the workspace panel composer appends those characters to the draft.
- Row navigation still works with arrow keys.
- Opening/attaching selected Pods remains reachable through the existing blank-Enter behavior where applicable.
- Refresh remains reachable through an explicit non-text path if one already exists; otherwise do not invent a new shortcut in this ticket unless needed for tests.
- Actionbar/help text matches the actual key behavior.
- Focused TUI tests cover the removed shortcuts and composer insertion behavior.
- `cargo test -p tui ... --lib`, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
