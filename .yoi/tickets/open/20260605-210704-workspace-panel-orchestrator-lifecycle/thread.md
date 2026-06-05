<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T23:32:34Z -->

## Plan

Preflight result: `implementation-ready` after the first action-model slice.

This ticket should add the workspace Orchestrator lifecycle to `yoi panel` only for Ticket-enabled workspaces. If `.yoi/ticket.config.toml` is absent, the panel should skip Orchestrator lifecycle and remain Pod-centric, matching the old `--multi` behavior.

Implementation should use existing Pod restore/spawn semantics and the Ticket role launcher/profile configuration where practical, surface bounded diagnostics, and leave final layout tuning for later.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
