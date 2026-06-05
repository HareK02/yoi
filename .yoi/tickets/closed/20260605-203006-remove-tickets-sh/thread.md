<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:52:55Z -->

## Plan

Preflight result: `implementation-ready` after storage migration.

`yoi ticket` has parity and active storage has moved to `.yoi/tickets/`, so `tickets.sh` should now be removed to eliminate the temporary second mutation path. Active docs/project instructions/tests should use `yoi ticket ...`; historical closed records and old report artifacts do not need mass rewriting.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-05T22:13:36Z status: approve -->

## Review: approve

External reviewer approved the implementation with one non-blocking stale phrase in `docs/development/work-items.md`.

The stale phrase was fixed before merge. After merge, the remaining top-level `work-items/README.md` legacy notice was also removed so the repository-root `work-items/` path is not present as an active or compatibility surface.

Review summary:
- `tickets.sh` is deleted.
- Active docs/project instructions/tests use `yoi ticket ...` and `yoi ticket doctor`.
- No production code or tests execute `./tickets.sh`.
- `.yoi/tickets/` remains the active Ticket storage root.
- Remaining references are historical records/spec context, not active command instructions.


---

<!-- event: close author: hare at: 2026-06-05T22:13:36Z status: closed -->

## Closed

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


---
