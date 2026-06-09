Implemented typed Ticket thread event logging for workflow audit events.

Changes:
- Added event kinds:
  - `state_changed`
  - `intake_summary`
- Preserved existing event kinds and historical compatibility.
- Added typed backend data/API:
  - `TicketStateChange`
  - `TicketIntakeSummary`
  - `TicketBackend::add_state_changed(...)`
  - `TicketBackend::add_intake_summary(...)`
  - `TicketBackend::set_state_field(...)` as a frontmatter-field update plus `state_changed` event helper for the next workflow-state slice.
- Parser now understands quoted event attributes and exposes typed metadata such as `from`, `to`, `reason`, `state_field`, and full attributes.
- Doctor validates required fields for `state_changed` and `intake_summary` where practical.
- `TicketShow` tool output includes new event metadata fields/attributes.
- Thread event append now prevalidates and prerenders metadata before opening/appending `thread.md`, preventing failed appends from corrupting the log.
- Create event author validation happens before writing a ticket record.
- Documentation now describes `thread.md` as append-only audit history, not current-state authority.

Validation after merge:
- `cargo test -p ticket thread`
- `cargo test -p ticket`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after fixing prevalidation/partial-append safety.

`explicit-ticket-workflow-state` can proceed next.
