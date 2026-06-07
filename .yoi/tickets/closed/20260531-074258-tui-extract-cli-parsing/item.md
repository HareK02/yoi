---
id: 20260531-074258-tui-extract-cli-parsing
slug: tui-extract-cli-parsing
title: 'TUI: extract CLI parsing from main.rs'
status: closed
kind: task
priority: P2
labels: [tui, cleanup]
created_at: 2026-05-31T07:42:58Z
updated_at: 2026-05-31T13:38:30Z
assignee: null
legacy_ticket: null
---

## Background

A read-only investigation of `crates/tui` found that the TUI crate's flat module list is not only cosmetic: `main.rs` has accumulated entrypoint, CLI parsing, mode dispatch, terminal setup, Pod connection, single-Pod event loop, key/mouse handling, and tests.

This ticket is the first, lowest-risk cleanup step: move command-line parsing types/functions/tests out of `main.rs` so `main.rs` starts becoming a thin entrypoint without changing runtime behavior.

## Requirements

- Extract the TUI CLI parsing code from `crates/tui/src/main.rs` into a focused module, likely `crates/tui/src/cli.rs`.
- Move the related parse tests with the parsing implementation.
- Keep external CLI behavior unchanged, including:
  - `insomnia pod ...` runtime dispatch;
  - `--pod`, `--session`, `-r`, `--multi`, profile/manifest-related arguments;
  - memory lint and other headless subcommands.
- Keep this ticket to mechanical extraction plus names needed for clarity; do not reorganize the broader TUI module tree.
- Do not rename the `tui` crate/package.

## Non-goals

- Moving terminal runtime/event-loop code.
- Moving `ui::Mode` / view state.
- Reworking CLI UX or flag semantics.
- Introducing compatibility aliases for removed commands.

## Acceptance criteria

- `main.rs` no longer owns the core CLI parse type/function/test definitions.
- `main.rs` uses the extracted CLI module for dispatch.
- Behavior remains unchanged for normal TUI launch, Pod runtime launch, restore/attach options, multi-Pod launch, and memory lint.
- Focused CLI parser tests pass.
- `cargo fmt --check`, relevant `cargo test -p tui`, `cargo check -p tui`, `./tickets.sh doctor`, and `git diff --check` pass.
