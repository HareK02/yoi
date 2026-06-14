# E2E evidence for Ticket 00001KV3BQ7Q3

Validation date: 2026-06-14
Worktree: `/home/hare/Projects/yoi/.worktree/00001KV3BQ7Q3-panel-e2e-evidence`
Branch: `impl/00001KV3BQ7Q3-panel-e2e-evidence`

## Summary

| Target merge behavior | Status | E2E scenario / assertion |
| --- | --- | --- |
| `802fa1f00f8725fe35336e083cd05652fee1409e` / `merge: rewind live refresh` | Pass for current fixture PTY E2E | `single_pod_rewind_picker_applies_without_escape_and_suppresses_duplicate_enter` spawns the real `yoi` binary under PTY, opens the rewind picker, applies the target without Esc/restart/restore, observes `rewind_applied` with restored composer text, and now also waits for the post-apply PTY stream to contain the unique live composer marker `rewind-live-refresh`. |
| `02311883f7cda116676d8e179a14ad0be9e7a244` / `merge: panel mouse selection` | Pass for current fixture PTY E2E | `panel_mouse_click_selects_row_without_dispatching_action` spawns the real `yoi panel` PTY path, injects SGR mouse click input, observes `selection_changed` for the clicked row, and asserts no `action_requested` event was emitted by click alone. |
| `db7bad7a64766c2039a4c10781801cb571027955` / `merge: panel quit latency` | Pass for bounded current fixture PTY E2E; original live-terminal latency remains outside this fixture | `panel_ctrl_c_exits_promptly_after_background_barrier` spawns the real `yoi panel` PTY path with a held `reload` background task, confirms that task is pending, sends Ctrl-C, and asserts clean process exit within `PanelHarness::default_exit_wait()` (1500 ms) plus `quit_requested` and `background_task_aborted { task: "reload" }` events. This guarantees that pending fixture background reload work is aborted and does not block quit past the threshold; it does not prove arbitrary live-terminal latency outside this fixture. |

## Commands and results

- `cargo fmt --check` — passed.
- `cargo test -p yoi-e2e --features e2e --no-run` — passed; built `yoi-e2e` unit/integration test executables.
- `cargo test -p yoi-e2e --features e2e` — passed: `yoi_e2e` unit test 1/1, `panel` integration tests 3/3, `rewind` integration test 1/1, doc-tests 0.
- `cargo check -p yoi-e2e -p yoi -p tui` — passed.
- `git diff --check` — passed.

## Residual gaps / non-claims

- These are automated fixture PTY confirmations only. They are not manual/live-terminal validation.
- The mouse path uses the harness's SGR mouse injection through a PTY. It confirms the real `yoi panel` process path receives and handles the encoded click as intended, but it is not a hardware/terminal-emulator compatibility matrix.
- The quit-latency assertion is bounded to the fixture's held `reload` task and 1500 ms threshold. It confirms pending fixture background work does not user-visibly block quit beyond that bound, but does not independently reproduce every historical live latency observation.
