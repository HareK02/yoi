<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:10:22Z -->

## Plan

Preflight result: `implementation-ready` with transitional root behavior.

`provider = "builtin:yoi_local"` should become the canonical backend spelling now, but this ticket must not move records yet. Until `migrate-ticket-storage-to-yoi-tickets` lands, missing config should continue to resolve the active root to `<workspace>/work-items` so existing Ticket tools/CLI remain usable.

Implementation should add provider parsing/diagnostics and update active docs/examples, while leaving storage migration and `tickets.sh` removal to follow-up tickets.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
