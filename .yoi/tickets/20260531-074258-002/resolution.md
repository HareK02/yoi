Extracted the current single-Pod TUI runtime/event-loop implementation out of `crates/tui/src/lib.rs`.

Implementation:
- Added `crates/tui/src/single_pod.rs`.
- Moved single-Pod attach/spawn/resume orchestration, fullscreen terminal helpers, event loop/drain logic, key/mouse handling, compact/rewind/queue/interrupt handling, and related tests into `single_pod.rs`.
- Left `lib.rs` as the TUI library façade/high-level dispatcher with module declarations, `LaunchOptions`, `LaunchMode`, and `launch(...)`.
- Preserved public `tui::launch`, `LaunchOptions`, and `LaunchMode` behavior for the `insomnia` crate.
- Did not split `multi_pod.rs`, move render helpers, redesign keybindings, rewrite App state, or change Pod protocol/profile/manifest semantics.

Review:
- External reviewer `tui-runtime-reviewer-20260531` approved implementation commit `4d89718`.
- Reviewer noted one non-blocking follow-up: the thin `run_multi` bridge now lives in `single_pod.rs`; acceptable for this extraction, but could move to a small launch/runtime coordination module if dashboard orchestration grows.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p tui`
- `cargo check -p tui -p insomnia` (passed with existing dead-code warnings)
- `./tickets.sh doctor`
- `git diff --check`
- `wc -l crates/tui/src/lib.rs crates/tui/src/single_pod.rs` showed `lib.rs` at 117 lines and `single_pod.rs` at 1765 lines.
