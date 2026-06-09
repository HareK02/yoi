Implemented, reviewed, merged, and validated.

Summary:
- Deprecated long-lived umbrella/progress-container Tickets in maintained workflow/docs guidance.
- Clarified that Tickets should be concrete work items that can be implemented, reviewed, validated, and closed on their own terms.
- Updated broad-request split guidance to use concrete implementable Tickets, durable split decisions, Objective context, and future non-hierarchical typed relations rather than umbrella containers.
- Explicitly disallowed hierarchy/container substitutes such as parent/child, sub-ticket, umbrella, part-of, and contains.
- Preserved the carve-out for concrete planning/design/investigation Tickets.
- Added existing umbrella retirement guidance and recorded a migration/close recommendation on `workspace-panel-orchestrator-queue-automation`.

Implementation:
- Coder commit: `1349a75 ticket: deprecate umbrella containers`
- Reviewer approved with no blocking findings.
- Merge commit: `ee41ed9 merge: deprecate umbrella tickets`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

`cargo check --workspace` was intentionally omitted because the merged implementation is docs/workflow/Ticket-thread-only and no Rust code/tests changed; Nix build covered package/resource integrity.