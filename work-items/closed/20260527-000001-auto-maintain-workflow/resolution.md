The old Auto Maintain workflow is retired and removed.

Resolution:

- Deleted `.yoi/workflow/auto-maintain.md`.
- Closed this Ticket as superseded by the newer Ticket-based orchestration workflow split:
  - `ticket-intake-workflow`
  - `ticket-orchestrator-routing`
  - `ticket-preflight-workflow`
  - `multi-agent-workflow`
- Updated `multi-agent-workflow` to point to Ticket Intake / Orchestrator Routing / Preflight instead of `$user/auto-maintain`.
- Updated `ticket-intake-workflow` to remove the obsolete auto-maintain connection.
- Updated `prompt-eval-metrics` so future prompt/workflow evaluation targets the current Ticket workflows or worktree workflow instead of `/auto-maintain`.

Rationale:

`auto-maintain` had become a broad and unstable WIP workflow with old assumptions around TODO/tickets and maintenance loops. Keeping it resident risks encouraging large implicit automation and bypassing the clearer gates now provided by Ticket Intake, Ticket Orchestrator Routing, Ticket Preflight, and Multi-agent Worktree Workflow.

Future maintainer/scheduler/lease behavior should be designed as explicit follow-up work, not revived through the deleted auto-maintain workflow.

Validation:

- `git diff --check`
- `./tickets.sh doctor`
- open workflow/docs search no longer finds `auto-maintain` references outside this closed historical Ticket context.
