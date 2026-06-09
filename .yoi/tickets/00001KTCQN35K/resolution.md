Completed the Yoi-local Ticket backend migration.

Child tickets completed:
- `yoi-ticket-cli-parity`: added top-level `yoi ticket` commands for create/list/show/comment/review/status/close/doctor against the Rust Ticket backend.
- `builtin-yoi-local-ticket-backend-config`: introduced canonical backend provider config `provider = "builtin:yoi_local"`.
- `migrate-ticket-storage-to-yoi-tickets`: moved active Ticket records from `work-items/` to `.yoi/tickets/` and made `.yoi/tickets` the default/configured root.
- `remove-tickets-sh`: deleted the transitional `tickets.sh` mutation path and updated active docs/instructions/tests to use `yoi ticket ...`.

Final authority model:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

- `yoi` binary + `crates/ticket` backend own active Ticket operations.
- Active storage is `.yoi/tickets/`.
- Repository-root `work-items/` is no longer present and is not an active mutable backend.
- `tickets.sh` is removed.
- Historical records may still mention old paths/commands as history, but active instructions and validation use `yoi ticket ...`.

Final validation points across the sequence included:
- `cargo test -p ticket`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`
- absence checks for `tickets.sh` and repository-root `work-items/` after final cleanup.
