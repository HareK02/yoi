Ticket local files backend is complete and merged.

Implementation:

- `740b017 feat: add local ticket backend`
- merge commit: `1041cdb merge: add local ticket backend`

Summary:

- Added a new low-level `ticket` workspace crate.
- Added typed Ticket domain types and `TicketBackend` trait.
- Added `LocalTicketBackend` over the current `work-items/` storage.
- Implemented list/show/create/add_event/review/set_status/close/doctor operations.
- Preserved current local storage layout and `tickets.sh` compatibility.
- Kept the implementation independent from `pod`, `tui`, Intake, Orchestrator routing, and scheduler code.
- Did not rename `work-items/` or remove `tickets.sh`.

Review:

- External sibling reviewer approved with no blockers.
- Non-blocker follow-ups recorded in the implementation report/review:
  - event references are modeled but not persisted/parsed in `thread.md` yet;
  - write paths should avoid emitting extension values that `tickets.sh doctor` rejects unless the format is intentionally extended;
  - backend lock coordinates Rust callers but not direct concurrent `tickets.sh` writes;
  - inherited `thread.md` `---` separator ambiguity remains.

Post-merge validation passed:

- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`
