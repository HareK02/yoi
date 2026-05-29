<!-- event: create author: tickets.sh at: 2026-05-29T01:02:00Z -->

## Created

Created by tickets.sh create.

---

<!-- event: close author: hare at: 2026-05-29T02:08:56Z status: closed -->

## Closed

---
id: 20260529-010200-tui-command-completion-apply
slug: tui-command-completion-apply
title: Apply command completions from keyboard
status: closed
kind: task
priority: P2
labels: [tui, commands, ux]
created_at: 2026-05-29T01:02:00Z
updated_at: 2026-05-29T02:08:56Z
assignee: null
legacy_ticket: null
---

## Background

The TUI command mode (`:`) can show completion candidates, but the candidates cannot currently be applied with keyboard completion keys such as Tab. Also, when there is an unambiguous or selected completion candidate, pressing Enter should be able to complete the command and execute it in one action.

This should make command mode behave like a small command palette rather than a read-only suggestion list.

## Requirements

- Add keyboard application for command completions in command mode.
  - Tab should apply the currently selected completion candidate when a candidate exists.
  - If there is no explicit selection but exactly one candidate exists, Tab should apply that candidate.
  - Applying a command completion should replace the command name prefix with the canonical command name and preserve/position trailing argument editing sensibly.
- Enter behavior should use completion when appropriate.
  - If the command input has completion candidates and the current command name is incomplete, Enter should apply the selected/unambiguous candidate and execute the completed command in one action when doing so yields a complete executable command.
  - If applying a completion only fills the command name and arguments are still required, Enter should complete the command name and keep command mode active with a helpful state/notice rather than executing an invalid command.
  - If no candidate applies, existing command execution/error behavior should remain.
- Completion selection/navigation should be keyboard-accessible.
  - Existing up/down behavior should not regress.
  - If Tab cycles candidates today for another completion surface, command mode should still have a clear apply path.
- Keep normal composer completion behavior unchanged.
  - This ticket is for `:` command mode completion, not file-ref/chip completion in normal input.
- Keep command execution local.
  - Commands must not be submitted as user messages.

## Acceptance criteria

- In command mode, typing a command prefix and pressing Tab fills the selected/unambiguous command completion.
- In command mode, typing a command prefix with a selected/unambiguous executable completion and pressing Enter completes and executes it in one action.
- Ambiguous completions do not execute the wrong command silently; they require selection or further typing.
- Commands requiring arguments are not executed with missing arguments just because Enter applied the command name.
- Existing command execution behavior for fully typed commands is unchanged.
- Normal composer/file-ref completion behavior is unchanged.
- Focused tests cover Tab apply, Enter complete-and-execute, ambiguous candidate handling, and argument-required behavior.
- `cargo fmt --check`
- Relevant TUI command tests, e.g. `cargo test -p tui command --no-default-features` or equivalent.
- `cargo check -p tui`

## Out of scope

- New commands.
- Fuzzy matching beyond current prefix/alias suggestions.
- Mouse selection in the completion popup.
- Normal input/file reference completion changes.
- Changing command registry semantics outside completion application.


---
