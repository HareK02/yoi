Migrated active Yoi local Ticket storage from repository-root `work-items/` to `.yoi/tickets/`.

Final storage/config behavior:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

- Missing Ticket config now defaults to `<workspace>/.yoi/tickets`.
- `provider = "builtin:yoi_local"` remains the canonical provider spelling.
- Legacy `kind = "local"` remains only a transitional alias and is not documented as the active path.
- Tracked Ticket records were moved to `.yoi/tickets/{open,pending,closed}/...`.
- `work-items/` now contains only a legacy README notice and is not a live mutable backend.
- `.yoi/ticket.config.toml` was added for this repository.
- `.yoi/memory/` was not migrated or touched.
- `tickets.sh` remains only as a transitional maintainer shim and now defaults to `.yoi/tickets`; `WORK_ITEMS_DIR` remains available for one-off legacy/recovery checks.
- The later workspace panel UI design update from `develop` was merged into the migration branch before final merge, so the design ticket content is preserved under `.yoi/tickets`.

Validation after merge:
- `cargo test -p ticket config`
- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p yoi ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `./tickets.sh doctor`
- scratch workspace `target/debug/yoi ticket create` creates under `.yoi/tickets` and does not create `work-items`
- `nix build .#yoi --no-link --print-out-paths`

A first post-merge attempt to run `target/debug/yoi ticket doctor` failed because the binary was stale and still expected the old config schema; rebuilding with `cargo build -p yoi` fixed it. This was a local validation-order issue, not a source failure.

External review approved with no requested changes.

`remove-tickets-sh` can proceed next.
