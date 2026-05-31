<!-- event: create author: tickets.sh at: 2026-05-31T07:42:58Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-05-31T13:45:04Z status: approve -->

## Review: approve

External reviewer: `tui-view-mode-reviewer-20260531`
Reviewed implementation commit: `bc31bfa` (`tui: move view mode state`)
Verdict: approve

Summary:
- Moved the single-Pod history display mode type from `ui.rs` to `view_mode.rs`.
- Kept the type name `Mode` and preserved variants/methods (`Detail`, `Normal`, `Overview`, `cycle()`, `label()`).
- Updated `app.rs`, `ui.rs`, and `tool.rs` to import `crate::view_mode::Mode`.
- Removed the state-model dependency from `app.rs` to the render module for this type.

Requirements mapping:
- `Mode` now lives in a focused state-oriented module.
- `app.rs -> ui.rs` dependency caused by `Mode` is removed.
- Rendering/key/command behavior is unchanged by inspection because the enum and methods are a straight move.
- No broad `App` privatization, render-tree move, app split, crate/package rename, or unrelated runtime extraction was introduced.
- No active `crate::ui::Mode` / `ui::Mode` references remain under `crates/tui/src`.

Blockers: none.
Non-blocking follow-ups: none.

Validation adequacy:
- Coder validation covered fmt, focused/full TUI tests, TUI check, doctor, diff-check, and `ui::Mode` grep.
- Reviewer additionally performed read-only diff/reference checks and found no issues.


---
