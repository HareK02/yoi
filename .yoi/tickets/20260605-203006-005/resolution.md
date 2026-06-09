Yoi ticket CLI parity is complete and merged.

Implementation:

- `4d5068b feat: add yoi ticket CLI`
- merge commit: `4ba1b2f merge: add yoi ticket cli`

Summary:

- Added `yoi ticket ...` subcommands to the product `yoi` binary.
- Implemented:
  - `yoi ticket create`
  - `yoi ticket list`
  - `yoi ticket show`
  - `yoi ticket comment`
  - `yoi ticket review`
  - `yoi ticket status`
  - `yoi ticket close`
  - `yoi ticket doctor`
- The CLI uses `crates/ticket` backend APIs directly.
- The CLI does not shell out to `tickets.sh`.
- Active storage remains unchanged for this ticket: absent `.yoi/ticket.config.toml`, the backend defaults to `<workspace>/work-items`.
- `yoi ticket status ... closed` is intentionally rejected with guidance to use `yoi ticket close`, because status-only close would not write `resolution.md`.

Review:

- External sibling reviewer approved with no blockers.
- Non-blocker follow-ups:
  - `yoi ticket doctor` currently hides warning-only diagnostics when there are no errors.
  - `show`, `list`, and doctor diagnostic output are not explicitly bounded.
  - Body-source error text is generic and can mention `--resolution` for comment/review commands.

Post-merge validation passed:

- `cargo test -p yoi ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`

This clears the CLI prerequisite for the next migration step: `builtin-yoi-local-ticket-backend-config`.
