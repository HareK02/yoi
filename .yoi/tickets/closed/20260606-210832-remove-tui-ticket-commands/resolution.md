Removed the obsolete single-Pod TUI `:ticket ...` command surface.

Changes:
- Removed TUI command registration/parsing/handling for `:ticket intake`, `:ticket route`, `:ticket investigate`, `:ticket implement`, and `:ticket review`.
- Removed `CommandAction::TicketRole`, `TicketRoleCommand`, pending command-action plumbing, and the single-Pod runtime Ticket role launch path.
- Kept shared `client::ticket_role` launcher code because `yoi panel` uses it for Orchestrator/Intake launches.
- Kept `yoi panel`, `yoi ticket ...`, Ticket tools, and workflows intact.
- Updated active docs to direct users to `yoi panel`, Ticket tools/workflows, and `yoi ticket ...` instead of `:ticket ...`.
- Left an explicit unsupported note for single-Pod `:ticket ...` command mode.
- Updated parser tests so `ticket intake ...` is unknown/unsupported.
- Did not reintroduce `--multi`.

Validation after merge:
- `cargo test -p tui command`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p yoi panel`
- `cargo test -p client ticket_role`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

Remaining active-reference matches are intentional: `yoi ticket review`, panel/client `ticket_role` usage, unsupported-note/test coverage, historical report text, and non-command identifiers such as `ticket_enabled` / `builtin:ticket`.

External review approved with no requested changes.
