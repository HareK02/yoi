Implementation report for Ticket 00001KV072V89

Summary:
- Added panel row mouse hit testing in `crates/tui/src/multi_pod.rs` by recording selectable row rectangles during list rendering.
- Handled left mouse button down events as selection-only operations for ticket/pod rows; non-row clicks are no-ops and diagnostic popups ignore clicks behind them.
- Kept action execution on existing keyboard/action paths: clicked rows feed existing selected row behavior, including blank Enter, actionbar target status, and row detail selection.
- Added focused tests for row hit testing, click selection, non-row click no-op/draft preservation, and composer/keyboard behavior preservation after click selection.

Validation:
- `cargo fmt -p tui` — passed.
- `cargo fmt --check` — passed.
- `cargo test -p tui mouse_ --lib` — passed (4 tests; includes the focused mouse tests plus existing wheel capture test matching the filter).
- `cargo test -p tui row_hit_testing_maps_only_visible_selectable_rows --lib` — passed.
- `cargo check -p tui --all-targets` — passed.
- `git diff --check` — passed.
- Additional broader check: `cargo test -p tui multi_pod --lib` ran; 85 tests passed and 1 existing/unrelated test failed (`multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`, expected orchestration root vs `/repo/yoi`). The failure is outside the panel mouse selection change area.

Commit:
- `452c9df1` (`tui: select panel rows by mouse`)

Residual risks:
- Hit boxes are tied to the last rendered list area. This matches the current draw/event loop model; resize events trigger redraw, but stale hit boxes before the first draw would be ignored/empty.
- MVP intentionally handles left-button down only; hover, release-to-action, detailed pane clicks, and drag/text selection remain out of scope.
