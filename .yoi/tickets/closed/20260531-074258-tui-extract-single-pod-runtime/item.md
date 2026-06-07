---
id: 20260531-074258-tui-extract-single-pod-runtime
slug: tui-extract-single-pod-runtime
title: 'TUI: extract single-Pod runtime loop from main.rs'
status: closed
kind: task
priority: P2
labels: [tui, cleanup]
created_at: 2026-05-31T07:42:58Z
updated_at: 2026-05-31T13:57:02Z
assignee: null
legacy_ticket: null
---

## Background

A read-only investigation of `crates/tui` found that `main.rs` remains large even after CLI parsing because it owns terminal setup, Pod connection orchestration, single-Pod event loop, key/mouse handling, stream drain behavior, and many tests.

After CLI parsing is extracted, this ticket should move the single-Pod runtime/event-loop responsibilities out of `main.rs` so the entrypoint mostly chooses a mode and delegates execution.

## Requirements

- Extract single-Pod TUI runtime/event-loop code from `main.rs` into a focused module, likely `single_pod.rs` or `runtime.rs`.
- Include the related key/mouse handling and stream/event drain helpers only where doing so keeps behavior-preserving extraction clear.
- Keep mode dispatch in `main.rs` thin: parse CLI, choose runtime mode, delegate.
- Preserve terminal setup/restore behavior, input queueing behavior, interruption handling, task reminders, manual rewind/compact handling, and Pod socket behavior.
- Keep the change behavior-preserving; do not redesign keybindings or event handling.
- Avoid broad module reorganization in the same ticket.

## Dependencies / sequencing

- Prefer doing this after `tui-extract-cli-parsing` so `main.rs` is less entangled.
- This can be done before or after `tui-move-view-mode-state`, but implementation should avoid increasing `app <-> ui` coupling.

## Non-goals

- Moving render helpers into `render/`.
- Splitting `multi_pod.rs`.
- Reworking `App` state ownership.
- Changing Pod protocol or TUI UX.
- Renaming the `tui` crate/package.

## Acceptance criteria

- `main.rs` no longer owns the bulk of the single-Pod event loop implementation.
- Normal single-Pod TUI launch behavior is unchanged.
- Existing queueing, interrupt, compact, rewind, and stream handling tests still pass.
- `main.rs` remains the binary entrypoint and high-level dispatcher.
- `cargo fmt --check`, relevant `cargo test -p tui`, `cargo check -p tui`, `./tickets.sh doctor`, and `git diff --check` pass.
