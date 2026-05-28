---
id: 20260528-233524-multi-pod-open-return
slug: multi-pod-open-return
title: Return to multi-Pod view after opening a Pod
status: open
kind: task
priority: P2
labels: [tui, pod, ux]
created_at: 2026-05-28T23:35:24Z
updated_at: 2026-05-28T23:35:24Z
assignee: null
legacy_ticket: null
---

## Background

`tui --multi` can open the selected Pod with `o`. The current implementation treats this as leaving the multi-Pod dashboard and tail-calling the normal single-Pod TUI. Once the single-Pod screen is detached with `Ctrl+D` / `Ctrl+C`, the process exits instead of returning to the multi-Pod view.

For now, no special "back mode" or dedicated back key is needed. The desired behavior is simpler: when the user opens a Pod from the multi-Pod dashboard, the normal single-Pod attach screen runs as a nested attach session. When that session exits normally by detach/quit (`Ctrl+D`, `Ctrl+C`, or equivalent normal exit), control returns to the multi-Pod dashboard.

This should be implemented by abstracting the TUI launch/control flow, not by adding protocol features or making the single-Pod App deeply aware of multi-Pod mode.

## Requirements

- `tui --multi` remains the entrypoint for the multi-Pod dashboard.
- In `tui --multi`, pressing `o` on a selected openable Pod opens the normal single-Pod conversation screen.
- When that single-Pod screen exits normally, TUI returns to the multi-Pod dashboard instead of exiting the process.
  - `Ctrl+D` / `Ctrl+C` detach/quit from the opened single-Pod screen should return to multi view.
  - Normal single-Pod launches such as `tui <pod>` / `tui --pod <name>` should continue to exit the process on `Ctrl+D` / `Ctrl+C`.
- Avoid introducing a dedicated back key or back mode for this ticket.
  - The caller loop determines whether a normal single-Pod exit returns to multi view or exits the process.
- Preserve multi-Pod dashboard state where practical.
  - The selected Pod name should remain selected after returning, if still visible.
  - The multi-Pod composer contents should be preserved across open/return.
  - The Pod list should refresh after returning so status changes are visible.
- Keep terminal handling clean.
  - Do not unnecessarily leave/re-enter alternate screen between multi view and the nested single-Pod screen if the existing terminal can be reused safely.
  - If reusing the same `Terminal`, ensure cursor/mouse/raw-mode cleanup remains correct on final exit and on errors.
- Error handling should be explicit.
  - If opening the selected Pod fails before the single-Pod screen starts, show a multi-view notice and keep the dashboard active.
  - If the single-Pod session exits with a fatal error, return that error or show a clear diagnostic according to the existing TUI error behavior; do not silently swallow fatal failures.
- Existing `tui --multi` direct send behavior, section layout, and separator fix must continue to work.

## Suggested implementation direction

- Split the current single-Pod attach runner into a reusable function that accepts an existing `Terminal` and returns when the attached screen exits.
- Change `run_multi()` from one-shot `multi_pod::run(...).await -> Open -> run_pod_name(...)` into a controller loop:

```text
loop:
  run multi dashboard with previous app state / selected Pod
  Quit => exit process
  Open(pod) => run single-Pod attach screen using the same terminal
               on normal exit, refresh dashboard and continue loop
```

- Avoid adding a new protocol method. This is local TUI orchestration.
- Avoid making `App` carry a generic `BackToMulti` mode unless it is strictly necessary; prefer caller-owned control flow.

## Acceptance criteria

- From `tui --multi`, pressing `o` opens the selected Pod's normal conversation screen.
- Pressing `Ctrl+D` / `Ctrl+C` in that opened screen returns to the multi-Pod dashboard.
- Starting a single-Pod TUI directly still exits on `Ctrl+D` / `Ctrl+C`.
- Returning to multi view preserves multi composer contents and selected Pod name when possible.
- Returning to multi view refreshes Pod status/list.
- Opening failure from multi view leaves the user in multi view with a visible notice.
- Existing multi-Pod tests still pass.
- Focused tests cover the controller/runner behavior where possible, especially distinguishing direct single-Pod launch from multi-owned nested launch.
- `cargo fmt --check`
- `cargo test -p tui multi --no-default-features` or equivalent focused tests.
- `cargo check -p tui -p client -p pod`

## Out of scope

- Dedicated back key.
- Per-Pod detail panes inside the multi-Pod dashboard.
- Protocol changes.
- Changing direct-send semantics.
- Changing Pod visibility/discovery rules.
