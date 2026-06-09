---
title: "Scroll TUI composer around cursor"
state: "closed"
created_at: "2026-05-29T01:02:00Z"
updated_at: "2026-05-29T02:08:04Z"
---

## Background

The TUI composer/input area has a fixed visible height. When the input buffer grows beyond the visible area (for example 10+ lines), the rendered text is clipped instead of scrolling to keep the cursor visible.

This makes editing long messages unreliable: the user can continue typing or moving the cursor, but the relevant lines may be outside the visible area.

## Requirements

- Implement cursor-based vertical scrolling for the normal composer input area.
- The visible viewport should follow the cursor line when the input has more lines than the allocated input height.
  - Moving the cursor above the viewport scrolls up.
  - Moving the cursor below the viewport scrolls down.
  - Typing new lines at the bottom keeps the cursor visible.
  - Deleting lines clamps the scroll offset to valid bounds.
- Preserve existing input behavior:
  - editing operations.
  - cursor movement.
  - selection/completion behavior for file refs if applicable.
  - queued input behavior.
  - command mode behavior unless command input shares the same rendering path and needs the same fix.
- The cursor's terminal position should correspond to the visible cursor location after scrolling.
- The implementation should not simply increase composer height or hide conversation content indefinitely.
- Keep visual separators/borders consistent with the existing TUI layout.

## Acceptance criteria

- A composer buffer longer than the visible input area renders a window around the cursor instead of clipping from a fixed origin.
- Cursor up/down/page movement updates the composer viewport correctly.
- Inserting/deleting lines keeps viewport bounds valid.
- Existing short single-line and small multi-line input rendering remains unchanged.
- Focused tests cover viewport calculation around cursor position and clamping.
- `cargo fmt --check`
- Relevant TUI focused tests, e.g. `cargo test -p tui input --no-default-features` or equivalent.
- `cargo check -p tui`

## Out of scope

- Resizable composer UX redesign.
- Mouse scrolling inside composer.
- Horizontal scrolling/wrapping redesign beyond what is needed to keep current behavior correct.
- Changing command completion behavior; see `20260529-010200-tui-command-completion-apply`.
