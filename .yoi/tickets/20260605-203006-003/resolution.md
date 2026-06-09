Removed the transitional `tickets.sh` mutation path.

Changes:
- Deleted repository-root `tickets.sh`.
- Updated active docs, workflows, project instructions, and tests from `./tickets.sh` to `yoi ticket ...` / `yoi ticket doctor`.
- Removed shell compatibility tests that executed the script and kept Rust backend / `yoi ticket` coverage.
- Removed the remaining top-level `work-items/README.md` legacy notice after merge so the repository-root `work-items/` path is no longer present.
- Preserved `.yoi/tickets/` as the active Ticket storage root.

Validation after merge:
- `cargo test -p ticket`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `test ! -e tickets.sh`
- active docs/code grep for `tickets.sh` / `./tickets.sh`
- `nix build .#yoi --no-link --print-out-paths`

Additional post-merge cleanup validation:
- `target/debug/yoi ticket doctor`
- `test ! -e tickets.sh`
- `test ! -e work-items`
- active docs/code grep for `tickets.sh` / `./tickets.sh`
- `git diff --check`

External review approved; a non-blocking stale "compatibility CLI" phrase was fixed before merge.

The umbrella `yoi-local-ticket-backend-migration` can be closed.
