<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:28:37Z -->

## Plan

Preflight result: `implementation-ready` with repository-record migration risk.

The provider-config prerequisite is complete. This ticket now owns the active storage move from `work-items/` to `.yoi/tickets/` and should make `.yoi/tickets` the default/configured built-in Yoi local backend root.

Important boundaries:
- no generated memory migration;
- do not read or edit `.yoi/memory/`;
- do not remove `tickets.sh` in this ticket, but update it as a transitional maintainer shim if it remains present;
- do not mass-rewrite historical thread prose solely because it mentions `work-items/`.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
