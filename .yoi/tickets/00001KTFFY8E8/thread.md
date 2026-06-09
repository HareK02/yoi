<!-- event: create author: yoi ticket at: 2026-06-06T22:13:01Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T22:14:29Z -->

## Plan

Created as a companion split from `explicit-ticket-workflow-state`.

This ticket owns making Ticket `thread.md` a concise typed append-only event log for workflow state transitions and Intake summaries, rather than a freeform transcript/comment sink. It should define/implement events such as `state_changed` and `intake_summary`, and provide backend APIs that keep frontmatter current state and thread transition events in sync.


---

<!-- event: plan author: hare at: 2026-06-06T22:16:04Z -->

## Plan

Preflight result: `implementation-ready` as the foundational backend/API slice before `explicit-ticket-workflow-state`.

This ticket should formalize Ticket `thread.md` as a concise typed append-only event log by adding state-transition and Intake-summary event types/APIs while preserving existing historical thread compatibility. It should not add workflow_state frontmatter yet; that is the next ticket.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T22:48:18Z status: approve -->

## Review: approve

External reviewer approved the implementation after one requested-changes cycle.

Review summary:
- `state_changed` and `intake_summary` are typed event kinds.
- Existing event kinds and historical compatibility are preserved.
- Thread event metadata is prevalidated/prerendered before append, so failed appends do not corrupt `thread.md`.
- Create event author validation happens before writing a ticket record.
- Parser/doctor understand the new event kinds and validate required fields where practical.
- No premature `workflow_state` authority or scheduler/state-machine behavior was introduced.


---

<!-- event: close author: hare at: 2026-06-06T22:48:18Z status: closed -->

## Closed

Implemented typed Ticket thread event logging for workflow audit events.

Changes:
- Added event kinds:
  - `state_changed`
  - `intake_summary`
- Preserved existing event kinds and historical compatibility.
- Added typed backend data/API:
  - `TicketStateChange`
  - `TicketIntakeSummary`
  - `TicketBackend::add_state_changed(...)`
  - `TicketBackend::add_intake_summary(...)`
  - `TicketBackend::set_state_field(...)` as a frontmatter-field update plus `state_changed` event helper for the next workflow-state slice.
- Parser now understands quoted event attributes and exposes typed metadata such as `from`, `to`, `reason`, `state_field`, and full attributes.
- Doctor validates required fields for `state_changed` and `intake_summary` where practical.
- `TicketShow` tool output includes new event metadata fields/attributes.
- Thread event append now prevalidates and prerenders metadata before opening/appending `thread.md`, preventing failed appends from corrupting the log.
- Create event author validation happens before writing a ticket record.
- Documentation now describes `thread.md` as append-only audit history, not current-state authority.

Validation after merge:
- `cargo test -p ticket thread`
- `cargo test -p ticket`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after fixing prevalidation/partial-append safety.

`explicit-ticket-workflow-state` can proceed next.


---
