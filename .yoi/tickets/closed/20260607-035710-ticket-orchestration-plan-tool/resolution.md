Implemented, reviewed, merged, and validated.

Summary:
- Added a lightweight Ticket-domain orchestration plan/note surface.
- Added per-Ticket typed JSONL artifact storage at `artifacts/orchestration-plan.jsonl`.
- Added backend append/query APIs for orchestration plan records.
- Added LLM tools `TicketOrchestrationPlanRecord` and `TicketOrchestrationPlanQuery`.
- Query is available in the read-only Ticket tool set; record creation is in the mutating/lifecycle Ticket tool set.
- Supported plan kinds cover ordering, dependency, conflict, waiting/capacity notes, and accepted-plan summaries.
- Updated Orchestrator routing workflow guidance to consult and record orchestration plan records before queued acceptance where relevant.

Implementation:
- Coder commit: `b28b775 ticket: add orchestration plan tools`
- Reviewer approved with no blocking findings.
- Merge commit: `b3eab44 merge: add ticket orchestration plan tools`

Validation after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
