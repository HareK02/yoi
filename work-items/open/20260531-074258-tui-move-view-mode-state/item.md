---
id: 20260531-074258-tui-move-view-mode-state
slug: tui-move-view-mode-state
title: TUI: move view mode state out of ui module
status: open
kind: task
priority: P2
labels: [tui, cleanup]
created_at: 2026-05-31T07:42:58Z
updated_at: 2026-05-31T07:42:58Z
assignee: null
legacy_ticket: null
---

## Background

A read-only investigation of `crates/tui` found that `app.rs` imports `crate::ui::Mode` while `ui.rs` imports and renders `App`. This is not a Rust module cycle, but it makes the state/render boundary conceptually circular: application state depends on a type owned by the rendering module.

This ticket separates the single-Pod display/history mode state from the render module before larger module grouping work.

## Requirements

- Move the single-Pod display/history mode type currently owned by `ui.rs` to a state-oriented location.
  - Acceptable destinations include `app.rs` or a focused module such as `view_mode.rs`.
  - If renaming, prefer a clearer name such as `ViewMode` or `HistoryMode`; keep the diff small if a rename would become noisy.
- Update `app.rs`, `ui.rs`, `tool.rs`, and tests to use the new location/name.
- Remove the `app.rs -> ui.rs` dependency caused only by this mode type.
- Preserve rendering behavior and keyboard/command behavior.
- Keep visibility changes minimal; do not combine this with broad `App` field privatization.

## Non-goals

- Moving the whole render tree into `render/`.
- Splitting `app.rs` broadly.
- Reworking mode semantics or UI layout.
- Renaming the `tui` crate/package.

## Acceptance criteria

- `app.rs` no longer imports `crate::ui` solely to access display/history mode state.
- Render code still consumes the mode state from the state-oriented module/type.
- Existing mode-related behavior is unchanged.
- Focused tests covering mode/key/render behavior pass.
- `cargo fmt --check`, relevant `cargo test -p tui`, `cargo check -p tui`, `./tickets.sh doctor`, and `git diff --check` pass.
