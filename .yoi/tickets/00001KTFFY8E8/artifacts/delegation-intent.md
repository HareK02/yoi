# Delegation intent: typed Ticket thread event log

## Classification

`implementation-ready` foundational backend/API slice before `explicit-ticket-workflow-state`.

The explicit workflow-state ticket needs a reliable audit/event path for state transitions. This ticket should first make the existing Ticket thread format more typed and useful without rewriting history or changing panel workflow semantics yet.

## Intent

Turn `thread.md` into a concise typed append-only event log for durable Ticket events, especially state transitions and Intake summaries. Keep current state in `item.md` frontmatter; keep full conversations in Pod/session logs.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/typed-ticket-thread-event-log`
- branch: `work/typed-ticket-thread-event-log`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Add/formalize typed event kinds for at least:
  - `state_changed`;
  - `intake_summary`.
- Preserve existing event kinds and historical compatibility:
  - `create`;
  - `comment`;
  - `plan`;
  - `decision`;
  - `implementation_report`;
  - `review`;
  - `status_changed`;
  - `close`.
- Add typed Rust data/API for appending a `state_changed` event with fields such as:
  - `from`;
  - `to`;
  - `actor` or `author`;
  - `reason`;
  - optional concise `summary` / refs body.
- Add typed Rust data/API for appending an `intake_summary` event with a bounded Markdown body and/or structured heading metadata.
- Keep `thread.md` append-only.
- Do not use thread events as the current-state authority; this ticket may add event APIs, but `explicit-ticket-workflow-state` owns current workflow frontmatter fields.
- If a minimal state-transition helper is added, keep it forward-compatible with frontmatter state updates; do not invent a separate scheduler/state machine.
- Update parser/doctor to understand the new event kinds and required attributes where practical.
  - `state_changed` should require `from`, `to`, `author/actor`, and `at` or equivalent.
  - `intake_summary` should require normal event metadata and a non-empty body where practical.
- Keep existing Ticket tools/CLI working.
- Avoid mass rewriting historical `thread.md` records.
- Update docs/comments that describe thread role/event meanings.

## Current code map

- `crates/ticket/src/lib.rs`
  - `TicketEventKind` currently supports `StatusChanged` and `Other`.
  - `LocalTicketBackend::append_thread_event(...)` writes HTML comment event metadata.
  - `parse_thread(...)`, `parse_event_comment(...)`, `doctor_thread_events(...)` parse/validate thread events.
  - `LocalTicketBackend::add_event(...)`, `review(...)`, `status(...)`, and `close(...)` are existing mutation paths.
- `crates/ticket/src/tool.rs`
  - `TicketComment` currently exposes only `comment`, `plan`, `decision`, `implementation_report` roles. Consider whether to expose new event kinds now or keep them backend-only for first slice.
- `crates/yoi/src/ticket_cli.rs`
  - Direct CLI paths for comment/review/status/close; update only if new event API needs direct maintainer exposure.
- `docs/development/work-items.md`
  - Update thread semantics if touched.

## Non-goals

- Implementing explicit `workflow_state` frontmatter fields; next ticket owns that.
- Replacing Pod/session logs.
- Capturing full Intake conversations in Ticket thread.
- Rewriting all historical thread files.
- Building scheduler/lease/queue behavior.
- Panel layout changes.

## Validation

Run at least:

- `cargo test -p ticket thread` or targeted new thread event tests;
- `cargo test -p ticket`;
- `cargo test -p yoi ticket` if CLI/tool-visible behavior changes;
- `cargo test -p pod ticket --lib` if Ticket tools change;
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
- new event kinds and metadata fields;
- backend APIs added;
- parser/doctor behavior;
- tool/CLI/doc updates, if any;
- historical compatibility behavior;
- validation results;
- whether `explicit-ticket-workflow-state` can proceed.
