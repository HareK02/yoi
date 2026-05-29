---
id: 20260529-031832-multi-pod-empty-enter-open
slug: multi-pod-empty-enter-open
title: Open selected multi-Pod entry on empty Enter
status: open
kind: task
priority: P2
labels: [tui, pod, ux]
created_at: 2026-05-29T03:18:32Z
updated_at: 2026-05-29T03:18:32Z
assignee: null
legacy_ticket: null
---

## Background

`tui --multi` currently uses `o` to open/attach the selected Pod entry. Enter is used to send the composer contents to the selected idle live Pod.

When the composer is empty, pressing Enter has no message to send. Treat that input as the same action as `o`: open the selected Pod entry in the single-Pod conversation screen.

This should make the multi-Pod dashboard feel more picker-like while preserving direct-send behavior when text is present.

## Requirements

- In `tui --multi`, pressing Enter with an empty composer opens the selected Pod entry, equivalent to pressing `o`.
- Pressing Enter with non-empty composer keeps the current behavior: send the composer contents to the selected eligible idle live Pod.
- Whitespace-only composer should be treated consistently with existing send behavior.
  - If current send trims/rejects whitespace-only input as empty, Enter should open.
  - If current send treats whitespace as input, preserve that existing behavior.
- Opening must use the existing open path.
  - Do not duplicate attach/open logic.
  - Existing return-to-multi behavior after detaching from the opened Pod must continue to work.
- Non-openable selected rows should behave like `o` currently behaves.
  - Show the same diagnostic/notice and remain in multi view.
- Do not change `o` key behavior.
- Do not change direct-send delivery semantics.

## Acceptance criteria

- `tui --multi`: empty composer + Enter returns the same outcome/action as `o` for an openable selected Pod.
- `tui --multi`: non-empty composer + Enter still direct-sends to the selected eligible idle live Pod.
- Empty Enter on a non-openable row shows the same diagnostic as `o`.
- Existing `o` behavior and return-to-multi behavior remain unchanged.
- Focused tests cover empty Enter open, non-empty Enter send, and non-openable empty Enter diagnostic.
- `cargo fmt --check`
- `cargo test -p tui multi --no-default-features` or equivalent focused tests.
- `cargo check -p tui`
- `git diff --check`

## Out of scope

- Changing direct-send eligibility.
- Adding a new keybinding.
- Changing single-Pod attach behavior.
- Changing multi-Pod row layout.
