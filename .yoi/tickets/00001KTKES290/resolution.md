Implemented, reviewed, merged, and validated.

Summary:
- Migrated current Ticket storage from `.yoi/tickets/{open,pending,closed}/<id-or-slug>/` buckets to flat `.yoi/tickets/<ticket-id>/` directories.
- Simplified current Ticket identity to opaque path-derived canonical IDs such as `YYYYMMDD-HHMMSS-NNN`; title remains user-facing display/search text, but slug/title words are not canonical identity.
- Simplified current frontmatter to use canonical `state` instead of independent `status` / `workflow_state` axes.
- Removed current authority for frontmatter `id`, `slug`, `kind`, unmanaged `labels`, `status`, and `workflow_state`.
- Preserved distinct `done` vs `closed` lifecycle semantics; close records `state: closed` and `resolution.md` without moving directories.
- Updated Ticket backend, CLI, built-in tools, panel/TUI, role/session claims, orchestration-plan references, workflows, docs, tests, and local Ticket records to the flat ID/state model.
- Migrated local Ticket records, including merge-boundary records created while the branch was under review.

Implementation:
- Coder commits included `191a875`, `591db3f`, `21114fd`, `8fe4b82`, `6ca27f3`, and `a6326a9`.
- Reviewer approved after fix loops.
- Merge commit: `634da5d merge: simplify ticket identity fields`.

Validation:
- Pre-merge and merge-boundary validation passed:
  - `cargo fmt --check`
  - `git diff --check`
  - `cargo run -q -p yoi -- ticket doctor`
  - `cargo test -p ticket`
  - `cargo check --workspace`
  - `nix build .#yoi`
- After runtime/tooling refresh, new tools also read the flat layout successfully and `cargo run -q -p yoi -- ticket doctor` reports `doctor: ok`.
