Implemented workspace panel Orchestrator lifecycle.

Changes:
- `yoi panel` now ensures a workspace Orchestrator only in Ticket-enabled workspaces.
- If `.yoi/ticket.config.toml` is absent, the panel skips Orchestrator lifecycle and remains Pod-centric.
- Existing malformed/unusable Ticket config paths surface diagnostics instead of silently falling back to no-Ticket mode.
- Orchestrator Pod name is derived from the workspace directory using ASCII-safe normalization and the `-orchestrator` suffix, capped at 80 chars.
- Already-live Orchestrator is reported as live.
- Restorable Orchestrator is restored through existing Pod restore semantics.
- Missing Orchestrator is spawned through the existing Ticket role launcher as `TicketRole::Orchestrator`.
- Lifecycle decisions use an exact-name authority path and do not depend on the capped UI `PodList` rows.
- Panel close does not stop the Orchestrator.
- Orchestrator is not selected as the default foreground target when it is the only row or when another Pod can be selected.
- Bounded lifecycle/config diagnostics are displayed in the panel.

Validation after merge:
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi`
- `cargo test -p client ticket_role`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after one requested-changes cycle.

Known follow-up:
- Layout/display tuning remains intentionally deferred until the first end-to-end panel flow exists.
- Composer target switching and Intake handoff are covered by follow-up child tickets.
