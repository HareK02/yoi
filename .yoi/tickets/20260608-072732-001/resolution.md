Implemented, reviewed, merged, and validated.

Summary:
- Added typed, durable, non-hierarchical Ticket relation metadata.
- Stored forward project-level relations as typed JSON artifacts at `.yoi/tickets/<ticket-id>/artifacts/relations.json`.
- Supported first-version relation kinds: `depends_on`, `blocks`, `related`, `supersedes`, and `duplicate_of`.
- Derived inverse views such as `blocked_by`, `dependency_of`, `superseded_by`, and `duplicated_by` from forward relations rather than storing inverse relation kinds.
- Added Ticket backend validation and `ticket doctor` diagnostics for relation artifacts, dangling references, self relations, duplicate relations, and bounded dependency/blocking cycles.
- Added TicketRelation LLM tools, CLI relation add/list surfaces, TicketShow/List relation metadata, and workspace panel unresolved blocker hints.
- Kept relation metadata distinct from OrchestrationPlan execution records and from Pod/session/worktree/runtime claims.

Implementation:
- Coder commit: `4601ad2 ticket: add typed relation metadata`
- Reviewer approved with no blocking findings.
- Merge commit: `2225311 merge: add typed ticket relation metadata`

Validation after merge:
- `cargo test -q -p ticket ticket_relations`
- `cargo test -q -p ticket queue_gate_rejects`
- `cargo test -q -p ticket doctor_validates_ticket_relations`
- `cargo test -q -p ticket ticket_relation_tools_record`
- `cargo test -q -p ticket ticket_tool_name_partitions_are_explicit`
- `cargo test -q -p yoi ticket_cli_records_lists_and_shows_relations`
- `cargo test -q -p tui workspace_panel_marks_ready_ticket_with_unresolved_relation_blocked`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
