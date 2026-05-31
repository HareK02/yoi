<!-- event: create author: tickets.sh at: 2026-05-31T07:42:58Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T13:46:30Z -->

## Plan

Preflight classification: implementation-ready with updated current-code scope.

The original ticket was written before `insomnia-crate-cli-owner`, when `crates/tui/src/main.rs` owned both product CLI and single-Pod runtime loop. That product CLI entrypoint has since moved to the `insomnia` crate and `tui` is now library-only. The remaining cleanup target is therefore the same runtime/event-loop code now living in `crates/tui/src/lib.rs`, not an obsolete `main.rs` file.

Updated intent:
- Move the single-Pod TUI runtime/event-loop implementation out of `crates/tui/src/lib.rs` into a focused TUI module, likely `single_pod.rs`.
- Keep `lib.rs` as the library façade: module declarations, launch API/types, and high-level dispatch.

Requirements:
- Extract single-Pod runtime functions from `lib.rs` into a focused module while preserving behavior.
- Include related helpers only where mechanical and clear: event loop, key/mouse handling, stream/event drain helpers, and per-run connection orchestration.
- Keep terminal setup/restore behavior, input queueing, interruption, task reminders, manual rewind/compact handling, and Pod socket behavior unchanged.
- Do not move render helpers, split `multi_pod.rs`, redesign keybindings, or perform unrelated TUI module cleanup.
- Keep public `tui::launch`, `LaunchOptions`, and `LaunchMode` behavior stable for the `insomnia` crate.

Current code map:
- `crates/tui/src/lib.rs`: library façade plus large single-Pod runtime loop and helpers after CLI ownership migration.
- `crates/tui/src/multi_pod.rs`: dashboard runtime; out of scope except imports needed by `lib.rs`.
- `crates/tui/src/app.rs`, `input.rs`, `ui.rs`, `tool.rs`, `view_mode.rs`: single-Pod runtime dependencies; should be imported by the extracted module as needed.

Critical risks:
- Over-extracting unrelated launch/multi-Pod code and increasing churn.
- Accidentally changing key/mouse/event handling semantics while moving functions.
- Creating circular module visibility or broad `pub` exposure. Prefer `pub(crate)` only where `lib.rs` must call into `single_pod`.

Intent packet for coder:

Intent:
- Extract the single-Pod runtime loop from current `tui/src/lib.rs` into a focused module and leave `lib.rs` as a thin façade/high-level dispatcher.

Invariants:
- Behavior-preserving move.
- `tui` remains library-only; `insomnia` remains CLI owner.
- No Pod protocol/profile/manifest changes.
- No broad TUI layout refactor beyond this extraction.

Validation:
- `cargo fmt --check`
- focused `cargo test -p tui` tests for queueing/interrupt/compact/rewind/stream handling if available
- full `cargo test -p tui`
- `cargo check -p tui -p insomnia`
- `./tickets.sh doctor`
- `git diff --check`


---
