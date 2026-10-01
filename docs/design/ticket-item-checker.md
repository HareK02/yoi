# Asynchronous Ticket item checker

Worker-originated `TicketCreate` and `TicketEditItem` mutations are checked after the authoritative SQLite save. The checker is advisory: its scheduling, model execution, result delivery, or failure never rolls back a Ticket mutation and never changes Ticket state, Queue eligibility, assignments, or implementation authority.

## Applied paths

The trigger is in the Workspace Server's authenticated Worker Ticket REST path. A complete, verified Runtime/Worker source header pair is required before a check is scheduled. Every successful Worker `Create` and `EditItem` operation schedules the persisted item revision; failed saves and thread/state events do not schedule work.

The browser's direct item-edit handler currently does **not** schedule this check because it has no originating Runtime Worker advisory destination. Browser edits must not be presented as checked. Adding that path later requires an explicit notification/audit destination rather than borrowing an unrelated Worker identity.

## Immutable input and revision

The Job input contains only:

- canonical and user-facing Ticket IDs;
- the exact saved item revision;
- exact saved title and body;
- whether the operation created or edited the item;
- changed-field flags and the necessary previous title/body for edited fields.

The revision is the latest `create` or `item_edit` event ID, matching `TicketDetail.item_revision`. Conversation history, user requests, agreements, Worker transcripts, and external facts are not captured. The generic Backend Job input limit is 128 KiB; an oversized exact snapshot is not truncated or misrepresented as checked and produces a server diagnostic after the Ticket remains saved.

## Execution and bounds

The trigger reserves durable intent synchronously after save, then dispatches without waiting for model completion. Concurrency pressure never rejects that intent: attempts above the four-Job cap remain durably `reserved`, and terminal attempt transitions plus restart recovery drain queued attempts through an atomic `reserved` → `dispatching` slot claim. It uses T-674's Backend-owned Job runner and an ordinary registered embedded Runtime Worker with `builtin:backend-job`. That profile selects the existing lightweight `codex-oauth/gpt-5.6-luna` model through normal model configuration and authentication. The checker has only `SubmitBackendJobResult`; it has no Ticket mutation, Queue, Worker management, conversation discovery, Web, Memory, or Workdir tools.

Each check is bounded to 4 concurrent Backend Jobs, 45 seconds, one attempt, an 8 KiB result, at most 8 findings, bounded finding fields, and the Backend Job runner's fixed input/instruction/delivery limits. The deterministic Ticket/revision Job ID suppresses duplicate execution. Generic Job and attempt records retain completed results and categorized dispatch, timeout, invalid-output, and other failures.

## Result and advisory behavior

`internal.ticket_item_checker` defines the text-only categories and exact structured output. Server validation denies unknown fields, rejects duplicate or excessive findings, bounds every string, and requires each quote to be an exact substring of the inspected saved title or body. Invalid output terminalizes the attempt with `invalid_result` rather than treating it as a clean check.

Before advisory delivery, the server reloads the Ticket and compares the current item revision with the immutable checked revision. Findings for the current revision are sent only to the authenticated source Worker over the existing tracked `Notify` path. A notification identifies itself as a post-save checker advisory, includes Ticket/revision, exact quote, reason, and correction/confirmation direction, and explicitly states that it is not a user request, approval gate, workflow blocker, or implementation instruction.

Clean results and stale results send no Worker input. Their durable delivery records are terminalized with distinct suppression categories so restart recovery cannot later emit or repeatedly reclaim them. Notification failures remain separate from the completed checker result. A stopped or missing destination is never recreated or started.

## Authoring responsibility

`common.tickets` separately instructs Ticket authors not to turn their own current-turn scope into durable requirements and not to invent user constraints. Those rules use the author's request context. The independent checker deliberately does not infer user intent or constraint provenance and does not certify overall specification correctness when it returns no findings.
