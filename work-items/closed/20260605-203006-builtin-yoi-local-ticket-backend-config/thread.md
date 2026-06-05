<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:10:22Z -->

## Plan

Preflight result: `implementation-ready` with transitional root behavior.

`provider = "builtin:yoi_local"` should become the canonical backend spelling now, but this ticket must not move records yet. Until `migrate-ticket-storage-to-yoi-tickets` lands, missing config should continue to resolve the active root to `<workspace>/work-items` so existing Ticket tools/CLI remain usable.

Implementation should add provider parsing/diagnostics and update active docs/examples, while leaving storage migration and `tickets.sh` removal to follow-up tickets.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-05T21:26:56Z status: approve -->

## Review: approve

External reviewer approved the implementation.

Review summary:
- No blocking issues found.
- `provider = "builtin:yoi_local"` is supported as the canonical backend spelling.
- Transitional `work-items` storage is preserved.
- Unsupported providers fail closed.
- Legacy `kind = "local"` is retained only as a documented transitional alias.
- Tests/docs were updated sufficiently for this ticket.


---

<!-- event: close author: hare at: 2026-06-05T21:26:56Z status: closed -->

## Closed

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


---
