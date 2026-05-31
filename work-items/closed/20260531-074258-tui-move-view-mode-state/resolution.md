Moved single-Pod view/history mode state out of the render module.

Implementation:
- Added `crates/tui/src/view_mode.rs`.
- Moved `Mode` from `ui.rs` to `view_mode.rs` without changing variants or methods.
- Updated `app.rs`, `ui.rs`, and `tool.rs` imports to use `crate::view_mode::Mode`.
- Removed the `app.rs -> ui.rs` dependency caused solely by `Mode`.
- Kept render/key/command behavior unchanged.

Review:
- External reviewer `tui-view-mode-reviewer-20260531` approved implementation commit `bc31bfa`.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p tui mode`
- `cargo test -p tui`
- `cargo check -p tui` (passed with existing dead-code warnings)
- `./tickets.sh doctor`
- `git diff --check`
- `rg "crate::ui::Mode|ui::Mode" crates/tui/src || true` produced no active source hits.
