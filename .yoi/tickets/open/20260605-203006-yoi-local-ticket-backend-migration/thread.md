<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T20:32:09Z -->

## Decision

Decision: migrate Ticket authority to the `yoi` binary and Yoi's built-in local backend.

Target state:

- Direct CLI operations use `yoi ticket ...`.
- Backend config uses `provider = "builtin:yoi_local"`.
- Active Ticket storage lives under `.yoi/tickets/`.
- `tickets.sh` is removed.
- Top-level `work-items/` is removed as active storage.

Rationale:

- Normal users should use TUI role actions, Ticket tools, workflows, and `yoi ticket ...`, not a shell script.
- Keeping `tickets.sh` as a live mutation path duplicates Ticket semantics and undermines the Rust backend as authority.
- `.yoi/tickets/` aligns Ticket records with `.yoi/workflow` and `.yoi/ticket.config.toml` as tracked project orchestration state.
- `work-items/` is legacy storage naming after the project concept was renamed to Ticket.

Migration should land in child tickets so the repository remains operable at each step.


---

<!-- event: plan author: hare at: 2026-06-05T20:32:09Z -->

## Plan

Plan:

1. `yoi-ticket-cli-parity`
   - Add `yoi ticket ...` operations over the Rust Ticket backend.

2. `builtin-yoi-local-ticket-backend-config`
   - Add canonical `provider = "builtin:yoi_local"` backend config and defaults.

3. `migrate-ticket-storage-to-yoi-tickets`
   - Move active Ticket records from `work-items/` to `.yoi/tickets/`.

4. `remove-tickets-sh`
   - Delete the shell compatibility CLI and update active docs/workflows/validation to `yoi ticket doctor`.


---
