Implemented canonical Yoi local Ticket backend provider config.

Final backend config schema:

```toml
[backend]
provider = "builtin:yoi_local"
root = "work-items"
```

`provider = "builtin:yoi_local"` is the canonical spelling. Existing `kind = "local"` config is accepted as a short transitional alias so existing local workspaces/tests do not break during the migration, but docs now show the provider spelling.

Default root behavior intentionally remains `<workspace>/work-items` for missing config and for this transitional provider config. This ticket did not move storage to `.yoi/tickets`; that is owned by the follow-up `migrate-ticket-storage-to-yoi-tickets` ticket.

Implementation commit was merged from branch `work/builtin-yoi-local-ticket-backend-config`.

Validation after merge:
- `cargo test -p ticket config`
- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p yoi ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `target/debug/yoi ticket doctor`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

External review approved with no requested changes.

`migrate-ticket-storage-to-yoi-tickets` can proceed next.
