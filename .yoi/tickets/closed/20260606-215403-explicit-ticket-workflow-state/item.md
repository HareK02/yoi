---
id: 20260606-215403-explicit-ticket-workflow-state
slug: explicit-ticket-workflow-state
title: Replace inferred panel Ticket state with explicit workflow state
status: closed
kind: task
priority: P1
labels: [ticket, tui, orchestration, panel, state]
created_at: 2026-06-06T21:54:03Z
updated_at: 2026-06-07T00:08:04Z
assignee: null
legacy_ticket: null
workflow_state: done
---

## Background

The first-pass workspace panel derives Ticket phase/action/status from labels, title text, readiness fields, and thread events such as `plan`, `implementation_report`, and `review`. That was useful for bootstrapping the panel, but it makes the panel infer workflow state instead of reading explicit Ticket state.

The desired model is simpler and more durable: Ticket records should carry a small explicit workflow state, while transient execution activity remains derived from live Pod/session state and is not persisted into Ticket frontmatter.

## Goal

Replace panel-inferred Ticket workflow state with explicit Ticket workflow fields and update the panel/Orchestrator/Intake flows to use those fields.

## Target state model

Use a small durable state machine:

```text
intake -> ready -> queued -> inprogress -> done
```

Meanings:

- `intake`: Intake Pod and user are clarifying/materializing the Ticket.
- `ready`: Intake is complete; the Ticket is ready for a human to queue.
- `queued`: Human queued the Ticket; Orchestrator may schedule it when resources/priority allow.
- `inprogress`: Orchestrator/coder/reviewer/validator work is underway. Review/rework loops stay inside this state.
- `done`: completed/closed.

`blocked` is not a workflow state. Blocking/user attention is an overlay such as `attention_required`.

`review` is not a workflow state. Review/rework/validation are runtime activity or thread events inside `inprogress`.

## Durable fields

Add explicit current-state fields to Ticket frontmatter, for example:

```yaml
workflow_state: intake | ready | queued | inprogress | done
attention_required: null | "..."
queued_by: null | "user"
queued_at: null | "2026-..."
```

Exact field names can be refined during implementation, but the semantics should remain:

- `workflow_state` is the durable Ticket workflow state.
- `attention_required` is a durable human-attention overlay, not a separate workflow state.
- `queued_by` / `queued_at` are durable facts recorded when the user queues a ready Ticket.

Do not persist `activity` in Ticket frontmatter. Current activity such as implementing/reviewing/validating should be displayed by combining Ticket state with live Pod/session/role-launch metadata and latest thread events.

## Panel display simplification

Once explicit workflow state exists, the panel row should stop showing multiple near-synonymous columns such as priority/action/status/phase. For Ticket rows, the main list should show only the durable state plus identity/title:

```text
<sel> <state> <slug-or-id> <title>
```

Actions such as `Queue`, `Defer`, or `Open` should move to the actionbar/key-hint area for the selected row, not occupy a permanent row column. Ticket priority and other metadata can appear in the detail pane when useful, not as primary list columns.

Pod rows should follow the same principle:

```text
<sel> <pod-state> <pod-name>
```

Pod operations such as send/open should also be shown as selected-row key hints/actions, not as a permanent noisy row column.

The panel composer/status area should also be simplified. Do not put verbose target/help/diagnostic text in the status bar. Keep the status bar concise and stable; put transient guidance/errors in the actionbar or detail/diagnostic area instead.

## Flow

- `intake -> ready` is completed through Intake Pod conversation and Ticket materialization.
- `ready -> queued` is the normal human panel action.
- `queued -> inprogress` is performed by Orchestrator scheduling.
- `inprogress -> done` is performed by Orchestrator/review/close flow when complete.

The panel action currently called `Go` should become `Queue`, because the user is queuing a ready Ticket rather than approving implementation details.

## Thread/event-log relationship

Current workflow state should live in frontmatter, but every workflow state transition should be explainable through a concise append-only thread event. The thread should become a typed event log, not a freeform conversation transcript.

This is split into companion ticket `typed-ticket-thread-event-log` so this Ticket can focus on current-state fields and panel semantics while the companion defines/implements the event-log API.

Desired split:

- `item.md` frontmatter: current workflow state authority.
- `item.md` body: current Ticket snapshot.
- `thread.md`: typed append-only events such as `state_changed`, `intake_summary`, `decision`, `implementation_report`, `review`, and `close`.
- Pod/session logs: full conversations/runtime transcript.

State mutations should eventually use backend APIs that update frontmatter and append a `state_changed` event as one logical operation. Intake should write a concise `intake_summary` instead of copying the full Intake conversation into the Ticket thread.

## Requirements

- Add explicit workflow state fields to the Ticket model/parser/writer and tool/CLI surfaces as needed.
- Migrate or default existing Tickets safely without relying on labels/title/thread-event heuristics as authoritative state.
- Update `yoi panel` to display `workflow_state` directly.
- Simplify Ticket rows to state + identity/title only: remove permanent priority/action/status/phase columns from the main row.
- Simplify Pod rows to pod-state + pod-name only: remove permanent action/kind columns from the main row.
- Move row-specific operations such as Queue/Defer/Open/Send to selected-row actionbar/key hints rather than always-visible row columns.
- Keep composer/status bar text concise; avoid verbose target/help/diagnostic clutter in the status bar.
- Put transient guidance/errors in the actionbar or diagnostic/detail area instead of the status bar.
- Remove or demote current panel heuristics that infer phase/action from labels, title text, `readiness`, `needs_preflight`, or thread event presence.
- Rename panel `Go` action to `Queue` and make it transition `ready -> queued`.
- Queue action must re-check current Ticket state before mutation.
- Queue action records a durable typed `state_changed` / decision event and sets `queued_by` / `queued_at` if those fields are adopted.
- Orchestrator should treat `queued` as schedulable and set `inprogress` when it starts work, with a typed state transition event once the companion event-log API exists.
- Intake should set `workflow_state = ready` when the Ticket is fully materialized and ready to queue.
- Done/close flow should set `workflow_state = done` or derive it consistently from close status.
- Do not store transient `activity` in Ticket frontmatter.
- Preserve no-Ticket workspace Pod-centric panel behavior.

## Non-goals

- Building the full typed thread event-log API; companion ticket `typed-ticket-thread-event-log` owns that, though this ticket should align with it.
- Persisting live Pod activity into Tickets.
- Reintroducing human approve/reject gates for every review loop.
- Reintroducing `--multi` or `:ticket`.
- Layout-only tuning unrelated to explicit state, except the required state-only row/statusbar simplification described above.

## Acceptance criteria

- New/updated Tickets can carry explicit workflow state.
- Panel rows show workflow state from Ticket fields rather than inferred phase/status.
- Ticket rows use a minimal `state + slug/id + title` shape; priority/action/status/phase are not permanent main-list columns.
- Pod rows use a minimal `pod-state + pod-name` shape; operations are shown as selected-row key hints/actions, not permanent main-list columns.
- Composer/status bar text is concise and does not contain verbose target/help/diagnostic clutter.
- Ready Tickets show `Queue` rather than `Go` as the selected-row actionbar/key-hint action.
- Queue action transitions only `ready -> queued` and rejects stale/invalid states.
- Review/rework activity does not create a separate workflow state; it remains `inprogress` plus runtime/thread detail.
- No persistent `activity` field is required for current Pod activity.
- Existing tests cover default/migration behavior, panel display, Queue dispatch, and stale-state rejection.
- Companion ticket `typed-ticket-thread-event-log` exists and captures the append-only state transition / Intake summary event-log work if not implemented in the same change series.
