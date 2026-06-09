Implemented, reviewed, merged, and validated.

Summary:
- Made `TicketList` a lightweight bounded overview/selection surface.
- Default/max limit is bounded; output includes short metadata such as canonical id, truncated title, state, updated timestamp, and hints.
- Tool output includes count/returned/truncated/limit metadata.
- `body`, thread/event text, artifact content/details, and resolution content are not emitted from `TicketList`.
- CLI `yoi ticket list` uses the same bounded policy and `--limit` support.
- `TicketShow` remains the detailed authority for routing/implementation/close decisions.

Implementation:
- Coder commit: `7368416 ticket: lighten ticket list output`
- Reviewer approved with no blocking findings.
- Merge commit: `b6ac36e merge: lighten ticket list output`
- Follow-up mechanical fix after ToolExecutionContext landed: `f2cdb6f fix: align ticket list tests with tool context`

Validation after merge:
- `cargo test -p ticket ticket_list_tool`
- `cargo test -p yoi ticket_cli_list`
- `cargo test -p yoi ticket_cli_create_list_show_comment_review_state_close_and_doctor`
- `cargo test -p ticket ticket_tools_create_list_show_and_doctor`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
