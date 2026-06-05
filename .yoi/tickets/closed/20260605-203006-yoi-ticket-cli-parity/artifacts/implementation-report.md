# Implementation report: yoi-ticket-cli-parity

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/yoi-ticket-cli-parity`
- Branch: `work/yoi-ticket-cli-parity`

## Commit

- `4d5068b feat: add yoi ticket CLI`

## Summary

Added `yoi ticket ...` subcommands to the product `yoi` binary crate, backed directly by the Rust Ticket backend APIs.

The implementation does not shell out to `tickets.sh`. Active storage remains unchanged for this ticket: when `.yoi/ticket.config.toml` is absent, the CLI uses the current default `<workspace>/work-items`.

## Implemented command surface

- `yoi ticket create --title <title> [--slug <slug>] [--kind <kind>] [--priority P2] [--label a,b]`
- `yoi ticket list [--status open|pending|closed|all]`
- `yoi ticket show <id-or-slug>`
- `yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)`
- `yoi ticket review <id-or-slug> (--approve|--request-changes) (--file <path>|--message <text>)`
- `yoi ticket status <id-or-slug> <open|pending|closed>`
- `yoi ticket close <id-or-slug> (--resolution <text>|--file <path>)`
- `yoi ticket doctor`

`yoi ticket status ... closed` is intentionally rejected with guidance to use `yoi ticket close`, because status-only close would not write `resolution.md`.

## Backend behavior

- Uses `ticket::config::TicketConfig::load_workspace(<cwd>)` to resolve backend root.
- Missing `.yoi/ticket.config.toml` defaults to `<cwd>/work-items`.
- Relative configured roots resolve against the workspace.
- Malformed config fails through the existing config loader.
- No storage migration to `.yoi/tickets` is included.

## Changed files

- `Cargo.lock`
- `crates/ticket/src/lib.rs`
- `crates/yoi/Cargo.toml`
- `crates/yoi/src/main.rs`
- `crates/yoi/src/ticket_cli.rs`
- `package.nix`

## Review status

External sibling reviewer approved with no blockers.

Non-blocker follow-ups:

- `yoi ticket doctor` currently hides warning-only diagnostics because it prints `doctor: ok` when `error_count() == 0`.
- `show`, `list`, and doctor diagnostic output are not explicitly bounded.
- Body-source error text is generic and can mention `--resolution` for comment/review commands.

## Validation

Coder-reported validation passed:

- `cargo test -p yoi ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `git diff --cached --check`
- `./tickets.sh doctor`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`

Reviewer-rerun validation passed:

- `git diff --check develop...HEAD`
- `./tickets.sh doctor`
- production CLI grep found no shell-out to `tickets.sh`.

## Ready for merge

Yes.
