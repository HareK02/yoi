<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T22:35:56Z -->

## Plan

Preflight result: `implementation-ready` as the first implementation slice after design approval.

Implementation should add a thin, testable workspace panel ViewModel/action model and integrate it enough into the current `--multi` dashboard to show Ticket/action rows above passive Pod rows. The model should be local-file-first from `.yoi/tickets/`, reuse existing Pod list data for background Pod state, avoid live socket I/O in the model layer, and leave final layout/display tuning to follow-up adjustments after the first end-to-end pass.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-05T23:31:28Z status: approve -->

## Review: approve

External reviewer approved current HEAD after requested changes were addressed.

Review summary:
- Spike classification is present and tested.
- Ordinary open backlog Tickets remain background/non-action and are not promoted to `Go` by default.
- Integration coverage verifies a Ticket action row above an idle Pod while preserving Pod open/direct-send behavior.
- The model remains plain UI/action data; rendering and live socket I/O stay in existing TUI/Pod paths.
- Ticket loading uses Rust Ticket config/backend APIs and does not shell out.
- `yoi panel` is wired as the launch path; `--multi` is no longer accepted.
- Missing Ticket config suppresses Ticket UI and leaves the panel Pod-centric.

Non-blocking follow-up: malformed Ticket config/read errors are currently silently ignored by the panel model. A later refinement should surface those through the existing diagnostics field when config exists but loading fails.


---

<!-- event: close author: hare at: 2026-06-05T23:31:28Z status: closed -->

## Closed

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


---
