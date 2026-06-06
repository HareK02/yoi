# Delegation intent: explicit Ticket workflow state

## Classification

`implementation-ready` after `typed-ticket-thread-event-log`.

The typed thread event foundation is complete. This ticket should add explicit durable workflow state to Ticket records and update the panel/queue action to use it instead of inferred phase/action/status heuristics.

## Intent

Replace inferred panel Ticket state with explicit Ticket workflow fields and simplify the panel main list to display durable state directly.

Durable workflow state:

```text
intake -> ready -> queued -> inprogress -> done
```

The only normal human gate is `ready -> queued`, exposed as `Queue` in the panel. Review/rework/validation remain within `inprogress`; transient activity should not be stored in Ticket frontmatter.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/explicit-ticket-workflow-state`
- branch: `work/explicit-ticket-workflow-state`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Add typed workflow fields to Ticket frontmatter/model parsing/writing:
  - `workflow_state: intake | ready | queued | inprogress | done`;
  - `attention_required: null | "..."` or compatible optional string overlay;
  - `queued_by: null | "user"` or compatible optional string;
  - `queued_at: null | timestamp` or compatible optional timestamp/string.
- Preserve existing Tickets through safe defaults/migration behavior.
  - Existing open Tickets without `workflow_state` should not be guessed from title/labels/thread as authoritative state.
  - Use a conservative default such as `intake` or `ready` only if documented and tested; if the current status is closed, map to `done` where appropriate.
- Use the typed thread event APIs from `typed-ticket-thread-event-log` for state changes where practical:
  - `ready -> queued` should update frontmatter and append `state_changed` as one logical mutation;
  - the event should record actor/reason and remain concise.
- Update `yoi panel` to display workflow state directly.
- Simplify panel main-list rows:
  - Ticket rows: `<sel> <state> <slug-or-id> <title>`;
  - Pod rows: `<sel> <pod-state> <pod-name>`;
  - remove permanent priority/action/status/phase columns from main rows.
- Move row-specific operations such as Queue/Defer/Open/Send to selected-row actionbar/key hints instead of always-visible row columns.
- Keep composer/status bar concise and stable; move verbose target/help/diagnostics to actionbar/detail/diagnostic area.
- Remove or demote heuristics that infer phase/action from labels, title text, `readiness`, `needs_preflight`, or thread event presence.
- Rename panel `Go`/`ApproveIntake` queue-like behavior to `Queue`.
- Queue action semantics:
  - only valid when current `workflow_state == ready`;
  - re-check state before mutation;
  - transition to `queued`;
  - set `queued_by` / `queued_at` if adopted;
  - append typed `state_changed` event;
  - notify Orchestrator if reachable, but notification failure must not roll back a successful Ticket transition.
- `Defer` may remain as a safe status/pending action if currently useful, but it should not pretend to be workflow state unless explicitly modeled.
- Intake readiness:
  - add API/tool/helper path for Intake/Orchestrator to mark a Ticket `ready` using explicit state and, where practical, an `intake_summary`/`state_changed` event;
  - do not dump full Intake transcripts into Ticket thread.
- Done/close flow should set or derive `workflow_state = done` consistently.
- No-Ticket workspace Pod-centric panel behavior must remain unchanged.
- Do not store transient `activity` in frontmatter.
- Do not reintroduce `--multi` or `:ticket`.

## Current code map

- `crates/ticket/src/lib.rs`
  - Ticket item/frontmatter parsing/writing, `Ticket` model, backend APIs.
  - Newly available typed event APIs: `TicketStateChange`, `TicketIntakeSummary`, `add_state_changed`, `add_intake_summary`, `set_state_field`.
- `crates/ticket/src/tool.rs`
  - Built-in Ticket tool behavior. Add workflow state support only if needed for Intake/Orchestrator/Panel flows.
- `crates/yoi/src/ticket_cli.rs`
  - Direct CLI. Update display/create behavior only if needed; keep CLI stable.
- `crates/tui/src/workspace_panel.rs`
  - Current inferred `TicketPanelPhase`, `NextUserAction`, row derivation heuristics.
- `crates/tui/src/multi_pod.rs`
  - Panel row rendering, Queue/Defer action dispatch, status/action bar text, Orchestrator notification.
- `crates/client/src/ticket_role.rs`
  - Intake/Orchestrator role launch context if state-ready/intake-summary integration needs launch-prompt changes.

## Non-goals

- Scheduler/lease/queue system.
- Persisting live Pod activity into Tickets.
- Reintroducing human approve/reject gates for every review loop.
- Reintroducing `--multi` or `:ticket`.
- Broad layout redesign beyond the required state-only row/statusbar simplification.

## Validation

Run at least:

- `cargo test -p ticket workflow` or targeted workflow-state tests;
- `cargo test -p ticket`;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi_pod`;
- `cargo test -p yoi panel`;
- `cargo test -p yoi ticket` if CLI behavior changes;
- `cargo test -p pod ticket --lib` if tools change;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `cargo build -p yoi`;
- `target/debug/yoi ticket doctor`.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final frontmatter fields and default/migration behavior;
- state transition API usage;
- Queue semantics and Orchestrator notification behavior;
- panel row/statusbar simplification;
- heuristics removed/demoted;
- tests updated/added;
- validation results;
- remaining follow-up items, if any.
