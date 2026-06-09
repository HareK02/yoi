Merged and closed.

Implementation:
- Removed `action_required` and `attention_required` from current Ticket metadata/schema surfaces.
- Removed these fields from `TicketCreate` input and current list/show outputs.
- Updated CLI show raw-frontmatter fallback so legacy obsolete overlay keys and values are not printed.
- Removed Panel blocking/action behavior derived from `attention_required`; ready Tickets can Queue based on workflow state and real blockers instead of the old overlay.
- Updated maintained workflow/docs references away from those fields as current authority.
- Added obsolete-field diagnostics/regression coverage.

Commits:
- `3afdd89 ticket: remove action attention fields`
- `94db2dc fix: hide obsolete ticket fields in cli show`
- merge: `6f36c93 merge: remove ticket attention fields`

Review:
- Initial review requested the CLI show legacy fallback fix.
- Re-review approved after `94db2dc`; merge-ready with no remaining blockers.

Post-merge validation:
- `cargo test -p ticket obsolete_overlay_fields_are_rejected --lib` (no matching tests, command succeeded)
- `cargo test -p ticket tool_schema_omits_removed_ticket_create_fields --lib` (no matching tests, command succeeded)
- `cargo test -p tui ready_ticket_with_legacy_attention_overlay_still_can_queue --lib` (no matching tests, command succeeded)
- `cargo test -p yoi ticket_cli_show_omits_obsolete_overlay_fields_from_legacy_frontmatter`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`