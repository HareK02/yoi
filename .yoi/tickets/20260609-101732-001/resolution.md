Implemented, reviewed, merged, and validated.

Summary:
- Updated Orchestrator routing guidance to prefer starting additional independent queued Tickets during explicit queue review/routing when capacity exists and safety checks pass.
- Preserved the rule that queue notifications are not unattended scheduler triggers.
- Preserved the durable `queued -> inprogress` acceptance gate before worktree creation, Pod spawn, or implementation side effects.
- Explicitly forbids starting unqueued Tickets to fill capacity.
- Added required parallel-start checks for Ticket body/thread/artifacts, Ticket relations, OrchestrationPlan, workspace/worktree dirty state, visible Pods, branches, and conflict/dependency notes.
- Added bounded idle/defer reasons when capacity appears available but queued work is left waiting.
- Updated multi-agent workflow guidance for separate worktrees/branches/narrow write scopes and read-only Reviewer default.

Implementation:
- Coder commit: `492fe06 workflow: prefer parallel queued starts`
- Reviewer approved with no blocking findings.
- Merge commit completed after ToolExecutionContext cleanup.

Validation after merge:
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

`cargo check --workspace` was intentionally omitted because the change is workflow-resource text only and Nix build covered packaged resource integrity.