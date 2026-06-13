Request changes.

Evidence reviewed:
- Inspected Ticket record and `git diff 134e8b8b..HEAD` for commits `96561897` and `10a1c383`.
- `tests/e2e` provides a credible first declarative harness (`PanelHarness::spawn`, `wait_for`, `wait_for_rows`, `click`, `press`, `expect_selection`, `expect_exit_within`, artifacts/metadata/input/output/event logs). This is not merely a fixed-sleep shell script.
- Mouse-selection scenario waits for rendered rows, verifies both normal mouse and SGR mouse capture before `click`, sends the click through PTY bytes, waits for `selection_changed`, and asserts no `action_requested` dispatch.
- Quit-latency scenario creates a real feature-gated background-task hold barrier, waits until the task is actually waiting before sending Ctrl+C through the PTY, and measures bounded exit latency.
- `yoi-e2e` is opt-in via package feature/test `required-features = ["e2e"]`; e2e tests are outside default members. `YOI_TUI_TEST_EVENTS` and `YOI_TUI_TEST_HOLD_BACKGROUND_TASK` env behavior is behind `tui/e2e-test` / `yoi/e2e-test` feature gates, and the hook is observability-only.

Required change:
- The normal production build still contains/evaluates too much e2e harness glue. In non-`e2e-test` builds, `crates/tui/src/e2e_observer.rs` exposes no-op `emit`/hold functions, but call sites still execute test-specific data construction. In particular `App::emit_rows_rendered` and its panel row key/rect DTOs are compiled unconditionally and `app.emit_rows_rendered()` is called from the panel render path, causing row snapshots to be built every draw even though emission is a no-op. Selection/action/quit call sites also construct `serde_json::json!` payloads before the no-op facade. This violates the recorded boundary that production binaries should not contain harness logic and production-side hooks must be feature-gated/compiled out for normal builds.
  - Please cfg-gate the call sites/helpers/DTOs, or use a lazy cfg-gated macro/helper so normal builds do not evaluate or retain e2e event payload construction. A tiny compile-only facade is acceptable only if it does not execute or allocate e2e-specific work and does not keep harness DTO logic in the normal runtime path.

Validation run in `/home/hare/Projects/yoi/.worktree/e2e-harness`:
- `git diff --check 134e8b8b..HEAD` — passed.
- `cargo fmt --check` — passed.
- `cargo check -p tui --all-targets` — passed.
- `cargo check -p yoi --all-targets` — passed.
- `cargo build -p yoi --features e2e-test` — passed.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-harness/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed.
- `cargo check -p tui --all-targets --features e2e-test` — passed.
- `cargo check -p yoi --all-targets --features e2e-test` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.

No source changes were made during review.
