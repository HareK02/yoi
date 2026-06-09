Merged and closed.

Implementation:
- Added shared `crates/project-record` helper for canonical project record IDs.
- Ticket and Objective creation now use the same fixed-width base32 Unix epoch millisecond ID allocator.
- IDs use alphabet `0123456789ABCDEFGHJKMNPQRSTVWXYZ` and width 13.
- Collision handling retries by `+1ms` path probe, bounded at 1000 attempts, with no suffix/counter/random tail.
- `created_at` / `updated_at` remain human-readable frontmatter fields; collision-adjusted ID timestamp is not silently written as `created_at`.
- Migrated current repository records: 173 Ticket directories and 1 Objective directory.
- Updated relation artifacts, orchestration-plan artifacts, Objective linked Tickets, docs/examples/tests, package metadata, and Nix cargo hash.
- Added audit mapping artifact at `.yoi/tickets/00001KTNMN02R/artifacts/id-migration-map.txt`.

Commits:
- `4203988 feat: unify project record ids`
- merge: `5f6c695 merge: unify project record ids`

Review:
- Reviewer approved with no request-change findings.
- Residual note: `id-migration-map.txt` intentionally retains old ID strings as audit evidence. It is under artifacts and does not affect schema/list/doctor behavior.

Post-merge validation:
- `cargo test -q -p project-record`
- `cargo test -q -p ticket`
- `cargo test -q -p yoi objective_cli`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo run -q -p yoi -- objective doctor`
- `cargo check --workspace`
- `nix build .#yoi --no-link`