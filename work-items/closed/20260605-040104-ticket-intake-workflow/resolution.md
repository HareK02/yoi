Ticket Intake Workflow is complete.

Implementation:

- commit: `1233833 workflow: add ticket intake`

Summary:

- Added `.yoi/workflow/ticket-intake-workflow.md` as a user-invocable/model-invocable workflow.
- Updated `docs/development/workflows.md` to include Intake clarification and Ticket terminology.
- The workflow defines Intake as the clarification/materialization boundary before Orchestrator routing.
- It requires duplicate/related Ticket checks, readiness classification, needs-preflight/risk flags, draft presentation, and explicit user agreement before official Ticket creation.
- It uses typed Ticket tools (`TicketList`, `TicketShow`, `TicketCreate`, `TicketComment`, `TicketDoctor`) and forbids arbitrary filesystem edits to `work-items/`.
- It explicitly excludes scheduling, Pod spawning, worktree creation, merge/close, unattended automation, and user-agreement-free Ticket creation.
- It documents secret/private-context non-persistence rules.

Review:

- External sibling reviewer approved with no blockers.
- Reviewer non-blocker about old `work items` wording in `docs/development/workflows.md` was fixed before commit.
- Follow-up: if a future `TicketUpdate` tool is added, refine the existing-Ticket update path to choose between `TicketUpdate` and `TicketComment`.

Validation passed:

- `git diff --check`
- `./tickets.sh doctor`
- targeted grep found no `WorkItem` / old system-name wording in the new workflow.

No code/package changes were made, so cargo/nix validation was not required.
