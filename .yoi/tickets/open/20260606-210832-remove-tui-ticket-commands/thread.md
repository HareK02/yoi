<!-- event: create author: yoi ticket at: 2026-06-06T21:08:32Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T21:09:49Z -->

## Plan

Preflight result: `implementation-ready` cleanup.

The workspace panel now owns the Ticket/Intake/Orchestrator user-facing route, so the old single-Pod TUI `:ticket ...` command family should be removed rather than kept as fallback. Keep the shared role launcher because `yoi panel` uses it; remove only the TUI command surface/runtime handling and active docs/tests.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
