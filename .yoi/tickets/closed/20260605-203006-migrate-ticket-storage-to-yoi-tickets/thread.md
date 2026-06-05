<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:28:37Z -->

## Plan

Preflight result: `implementation-ready` with repository-record migration risk.

The provider-config prerequisite is complete. This ticket now owns the active storage move from `work-items/` to `.yoi/tickets/` and should make `.yoi/tickets` the default/configured built-in Yoi local backend root.

Important boundaries:
- no generated memory migration;
- do not read or edit `.yoi/memory/`;
- do not remove `tickets.sh` in this ticket, but update it as a transitional maintainer shim if it remains present;
- do not mass-rewrite historical thread prose solely because it mentions `work-items/`.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: implementation_report author: hare at: 2026-06-05T21:43:54Z -->

## Implementation report

Implemented the local Ticket storage migration to `.yoi/tickets/`.

- Moved tracked `work-items/{open,pending,closed}` records to `.yoi/tickets/{open,pending,closed}`.
- Added `.yoi/ticket.config.toml` with `provider = "builtin:yoi_local"` and `root = ".yoi/tickets"`.
- Updated default config resolution, Pod feature fallback, CLI tests/help, docs, and the transitional `tickets.sh` shim.
- Left `work-items/README.md` as a non-active compatibility notice only.
- Validated with the requested cargo tests/checks, both doctors, scratch default-create check, and `nix build .#yoi --no-link`.


---

<!-- event: review author: hare at: 2026-06-05T21:51:58Z status: approve -->

## Review: approve

External reviewer approved the implementation at branch head `9d142ec`.

Review summary:
- Active Ticket records moved under `.yoi/tickets/`.
- `work-items/` is reduced to a non-active legacy README and is not a second mutable root.
- Missing config/default root resolves to `<workspace>/.yoi/tickets`.
- `.yoi/ticket.config.toml` uses `provider = "builtin:yoi_local"` and `root = ".yoi/tickets"`.
- `tickets.sh` defaults to `.yoi/tickets` as a transitional shim.
- Rust Ticket CLI/tools and Pod feature preserve config-root behavior and fail-closed diagnostics.
- `.yoi/memory/` was not migrated or touched.
- The latest workspace panel design ticket update from `develop` was preserved after migration.

No changes requested.


---

<!-- event: close author: hare at: 2026-06-05T21:51:58Z status: closed -->

## Closed

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


---
