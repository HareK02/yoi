Implemented the first workspace panel action/model slice.

Changes:
- Added `crates/tui/src/workspace_panel.rs` with a thin UI/action model:
  - `WorkspacePanelViewModel`
  - `WorkspacePanelHeader`
  - `PanelRow`
  - `PanelRowKey`
  - `TicketPanelEntry`
  - `ActionPriority`
  - `NextUserAction`
  - `TicketPanelPhase`
- Added local-file-first Ticket row derivation through Rust Ticket config/backend APIs.
- Ticket rows are gated on explicit workspace Ticket config. If `.yoi/ticket.config.toml` is absent, the panel suppresses Ticket UI and remains Pod-centric.
- Added simple first-slice heuristics for intake/user reply, ready-for-Go, review needed, close ready, blocked, active work, backlog, and spike needed/running.
- Ordinary open backlog Tickets remain background/non-action and are not promoted to `Go` by default.
- Integrated the model into the current multi-Pod dashboard substrate so Ticket/action rows appear above passive Pod rows while preserving Pod selection, attach/open, and direct-send behavior.
- Added `yoi panel` launch parsing and removed `--multi` as a user-facing launch route.

Validation after merge:
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi`
- `cargo test -p yoi panel`
- `cargo test -p yoi parse_multi_flag_is_not_a_launch_alias`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after fixes.

Known follow-up:
- Surface malformed Ticket config/read failures via panel diagnostics instead of silently falling back to Pod-only display when config exists but loading fails.
- Layout/display tuning is intentionally deferred until the end-to-end panel flow exists.
