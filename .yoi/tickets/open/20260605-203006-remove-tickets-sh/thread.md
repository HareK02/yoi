<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T21:52:55Z -->

## Plan

Preflight result: `implementation-ready` after storage migration.

`yoi ticket` has parity and active storage has moved to `.yoi/tickets/`, so `tickets.sh` should now be removed to eliminate the temporary second mutation path. Active docs/project instructions/tests should use `yoi ticket ...`; historical closed records and old report artifacts do not need mass rewriting.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
