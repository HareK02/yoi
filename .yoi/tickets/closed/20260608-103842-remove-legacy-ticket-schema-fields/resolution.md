Implemented and merged.

Summary:
- Removed `legacy_ticket` from the current Ticket schema, metadata, summary, tool input/output, and CLI show/list/create surfaces.
- Removed `needs_preflight` from current schema/API/panel/tool output surfaces.
- Removed legacy `workflow_state: intake` compatibility; parser/tests now reject `intake` instead of normalizing it to `planning`.
- Migrated local `.yoi/tickets/**/item.md` frontmatter to remove obsolete `legacy_ticket` entries, including non-null values explicitly authorized as obsolete migration breadcrumbs.
- Preserved historical thread/resolution prose as audit history where it is not current schema/API input.

Implementation:
- Coder commit: `934a4b5 ticket: remove legacy schema fields`
- Merge commit: `de8e973 merge: remove legacy ticket schema fields`
- Reviewer approved with no blocking findings.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
