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

<!-- event: implementation_report author: hare at: 2026-06-05T21:43:54Z -->

## Implementation report

Implemented the local Ticket storage migration to `.yoi/tickets/`.

- Moved tracked `work-items/{open,pending,closed}` records to `.yoi/tickets/{open,pending,closed}`.
- Added `.yoi/ticket.config.toml` with `provider = "builtin:yoi_local"` and `root = ".yoi/tickets"`.
- Updated default config resolution, Pod feature fallback, CLI tests/help, docs, and the transitional `tickets.sh` shim.
- Left `work-items/README.md` as a non-active compatibility notice only.
- Validated with the requested cargo tests/checks, both doctors, scratch default-create check, and `nix build .#yoi --no-link`.


---
