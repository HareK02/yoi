# Implementation report: ticket-intake-workflow

## Summary

Added the user-invocable Ticket Intake workflow and lightly updated workflow development documentation.

The workflow defines Intake as the clarification/materialization boundary between a user's request and Orchestrator routing. Intake clarifies the request, checks duplicate/related Tickets, prepares a draft, obtains user agreement, and creates/updates Tickets through typed Ticket tools.

## Changed files

- `.yoi/workflow/ticket-intake-workflow.md`
- `docs/development/workflows.md`

## Workflow behavior

The new workflow covers:

- user intent clarification;
- duplicate/related Ticket checks with `TicketList` / `TicketShow`;
- requirements, acceptance criteria, non-goals, escalation conditions, validation, and related-work capture;
- explicit readiness classification:
  - `implementation_ready`;
  - `requirements_sync_needed`;
  - `spike_needed`;
  - `blocked`;
  - `unspecified` only with reason;
- `needs_preflight` and risk flag handling;
- user agreement before official Ticket creation;
- `TicketCreate` for new Tickets;
- `TicketComment` for existing Ticket refinement;
- fail-closed behavior when typed Ticket tools are unavailable;
- secret/private-context non-persistence rules;
- handoff to `ticket-preflight-workflow`, `multi-agent-workflow`, `auto-maintain`, and future `ticket-orchestrator-routing`.

## Non-goals preserved

The workflow explicitly avoids:

- scheduling implementation;
- spawning coder/reviewer/investigator Pods;
- creating worktrees;
- merge/close/branch cleanup;
- unattended automation;
- user-agreement-free Ticket creation;
- arbitrary filesystem writes to `work-items/`.

## Review status

External sibling reviewer approved with no blockers.

Reviewer non-blocker about an old `work items` phrase in `docs/development/workflows.md` was fixed before commit.

Remaining follow-up:

- If a future first-class `TicketUpdate` tool is added, the existing-Ticket refinement path should decide when to use it versus `TicketComment`.

## Validation

Validation passed:

- `git diff --check`
- `./tickets.sh doctor`
- targeted grep found no `WorkItem` / old system-name wording in the new workflow.

No code/package changes were made, so `cargo check`, `cargo fmt`, and `nix build` were not necessary for this workflow/docs-only change.
