<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T04:24:20Z -->

## Plan

Preflight result: `implementation-ready`.

The Ticket naming and umbrella split are accepted. This ticket is scoped to the first implementation layer only: typed Ticket domain/backend code and LocalTicketBackend compatibility with current `work-items/` files.

Key decisions for implementation:

- Use `Ticket` as the public/domain concept name.
- Keep `work-items/` as the current local storage path.
- Add a lower-level Rust crate for the Ticket backend layer; do not put this in `pod` or `tui`.
- Preserve `tickets.sh` compatibility and do not remove or replace the script in this ticket.
- Treat readiness/action-required/risk fields as optional/extensible because existing tickets do not have a fully normalized schema.
- Keep Markdown/freeform bodies; enforce mechanical consistency and safe mutation, not rigid body sections.
- No Pod tools, Intake workflow, Orchestrator routing, TUI UI, external tracker backend, or scheduler in this ticket.

The detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
