<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T22:35:56Z -->

## Plan

Preflight result: `implementation-ready` as the first implementation slice after design approval.

Implementation should add a thin, testable workspace panel ViewModel/action model and integrate it enough into the current `--multi` dashboard to show Ticket/action rows above passive Pod rows. The model should be local-file-first from `.yoi/tickets/`, reuse existing Pod list data for background Pod state, avoid live socket I/O in the model layer, and leave final layout/display tuning to follow-up adjustments after the first end-to-end pass.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
