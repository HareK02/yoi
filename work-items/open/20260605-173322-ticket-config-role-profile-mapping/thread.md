<!-- event: create author: tickets.sh at: 2026-06-05T17:33:22Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T17:35:08Z -->

## Decision

Decision: implement `.yoi/ticket.config.toml` as Ticket orchestration configuration with fixed Ticket role slots.

Use fixed roles, not an arbitrary Role registry:

- intake
- orchestrator
- coder
- reviewer
- investigator

The config maps these fixed Ticket roles to Profile selector strings and optional role system instruction / launch prompt / workflow refs. This keeps Profile as the Pod runtime recipe while Ticket orchestration owns the role-to-profile binding.

The first implementation should parse/validate config and wire the configured backend root into Ticket tools. It should not spawn Pods, add TUI actions, or implement a stateful workflow engine yet.

Detailed investigation and implementation plan: `artifacts/investigation-plan.md`.


---

<!-- event: plan author: hare at: 2026-06-05T17:35:08Z -->

## Plan

Plan:

1. Add Ticket config model/parser, probably in `crates/ticket/src/config.rs`, using lightweight string wrapper types for Profile/prompt/workflow refs so `ticket` does not depend on `pod`.
2. Parse `.yoi/ticket.config.toml` from a workspace root, with defaults when missing.
3. Support `[backend]` local root and fixed `[roles.*]` sections.
4. Wire the configured backend root into `crates/pod/src/feature/builtin/ticket.rs` so Ticket tools no longer hard-code `<workspace>/work-items`.
5. Add focused tests in `ticket` and `pod`.
6. Defer Pod role launching, TUI actions, and workflow-state/prompt sequencing to follow-up tickets.


---
