<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-06-05T06:10:31Z status: approve -->

## Review: approve

# Review: ticket-intake-workflow

## 1. Result: approve

Approve. I found no blockers for commit/close.

## 2. Summary

The new `ticket-intake-workflow` satisfies the ticket requirements and fits the current multi-agent direction. It defines Intake as a clarification/materialization boundary before Orchestrator routing, keeps implementation scheduling out of scope, requires user agreement before official Ticket creation, and uses typed Ticket tools instead of arbitrary `work-items/` file writes.

The `docs/development/workflows.md` update is small and accurate for the new workflow theme and child-Pod review responsibilities.

## 3. Requirement assessment

- Ticket terminology: pass. The new workflow consistently uses `Ticket` terminology and does not introduce WorkItem or insomnia wording. The only `work-items/` mention is the explicit prohibition against arbitrary filesystem edits, which matches the ticket authority-boundary requirement.
- Intake responsibilities: pass. The workflow covers intent clarification, duplicate/related Ticket checks, relevant repository reading when needed and permitted, clarifying questions, draft title/kind/priority/labels, and Ticket creation/update reporting.
- Intake non-goals: pass. The workflow explicitly forbids coder/reviewer/investigator Pod spawning, worktree creation, merge/close/branch cleanup, unattended scheduling, and implementation routing from Intake.
- User agreement: pass. It requires a pre-creation draft and official Ticket creation only after explicit approval or an explicit create/record request.
- Typed Ticket tools: pass. It directs agents to use `TicketList`, `TicketShow`, `TicketCreate`, `TicketComment`, and `TicketDoctor`, and says not to fall back to filesystem writes when tools are unavailable.
- Routing fields: pass. Readiness classification, `needs_preflight`, risk flags, acceptance criteria, escalation conditions, validation, and related work are represented in both the draft and recommended Ticket body.
- Secret/private handling: pass. It explicitly forbids persisting API keys, tokens, credentials, secret file contents, private prompts/responses, unnecessary private context, and raw secret-bearing logs.
- Workflow connections: pass. It connects to `ticket-preflight-workflow` for risk/uncertainty, `multi-agent-workflow` after implementation readiness, `auto-maintain` as an intake source, and future `ticket-orchestrator-routing` as the next routing layer.
- Docs update: pass. The docs change adds Intake to current workflow themes and updates child-Pod verification language from old ticket/work-item wording to Ticket requirements and acceptance criteria.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- Note: `docs/development/workflows.md` still contains the pre-existing phrase "replacement for work items" in the general workflow-role paragraph. The implementation diff did not introduce it and the changed lines use Ticket terminology, so I do not consider it a blocker. If the documentation pass is intended to eliminate all old wording globally, that line can be normalized in a follow-up.
- Future follow-up: if a first-class `TicketUpdate` tool becomes part of the typed Ticket surface, the existing-Ticket refinement path should decide when to use it versus `TicketComment`. The current workflow is acceptable because it avoids arbitrary file writes and records refinements through typed Ticket comments.

## 6. Validation assessed

Reviewed:

- `work-items/open/20260605-040104-ticket-intake-workflow/item.md`
- `.yoi/workflow/ticket-intake-workflow.md`
- `docs/development/workflows.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`

Checks run:

- `git diff -- .yoi/workflow/ticket-intake-workflow.md docs/development/workflows.md work-items/open/20260605-040104-ticket-intake-workflow/item.md`
- `git diff --check -- .yoi/workflow/ticket-intake-workflow.md docs/development/workflows.md work-items/open/20260605-040104-ticket-intake-workflow/item.md` — passed
- `./tickets.sh doctor` — passed
- Targeted terminology grep for `WorkItem`, `insomnia`, `work item`, `work-items`, and `Ticket`

I did not run `cargo check`, `cargo fmt`, or `nix build` because this review found only workflow/docs changes and no code/package changes to validate.

## 7. Residual risk

The main residual risk is dependency timing: the workflow assumes typed Ticket tools are available, while the ticket states those tools/backend are dependencies. The workflow handles unavailable tools by failing closed and returning to a human/parent workflow, so this is an integration dependency rather than a blocker in the workflow text.


---

<!-- event: implementation_report author: hare at: 2026-06-05T06:10:32Z -->

## Implementation report

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


---

<!-- event: close author: hare at: 2026-06-05T06:10:56Z status: closed -->

## Closed

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


---
