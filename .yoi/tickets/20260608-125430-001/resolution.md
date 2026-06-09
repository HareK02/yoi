Implemented, reviewed, merged, and validated.

Summary:
- Added first lightweight Objective project-record surface.
- Objective records live at `.yoi/objectives/<objective-id>/item.md` with path-derived opaque Objective IDs.
- Objective frontmatter includes `title`, `state: active|paused|done|archived`, `created_at`, `updated_at`, and `linked_tickets` using canonical Ticket IDs.
- Objective body includes required sections for goal, motivation/background, strategy/design direction, success criteria/exit conditions, and decision context.
- Added top-level `yoi objective` CLI: `create`, `list`, `show`, and `doctor`.
- Objective doctor validates malformed frontmatter/state/timestamps, missing required sections, unsafe IDs, and missing linked Ticket records.
- Objective-to-Ticket links are non-blocking context links and do not write Ticket relation metadata, drive Ticket state, or replace OrchestrationPlan execution records.
- Updated Intake/Orchestrator/work-item guidance to keep Objectives distinct from Tickets, TicketRelation metadata, OrchestrationPlan records, and Pod/session claims.

Implementation:
- Coder commit: `be12072 objective: add lightweight records`
- Reviewer approved with no blocking findings.
- Merge commit: `be69a8b merge: add objective records`

Validation after merge:
- `cargo test -p yoi objective_cli`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- objective doctor`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
