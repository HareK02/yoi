<!-- event: create author: tickets.sh at: 2026-06-05T19:03:30Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T19:06:16Z -->

## Plan

Preflight result: `implementation-ready`.

Implement `ticket-role-pod-launcher` before TUI actions so TUI does not own role/profile/prompt/workflow launch construction.

Design decisions:

- Fixed Ticket roles only: intake, orchestrator, coder, reviewer, investigator.
- Role profile/workflow/launch_prompt refs come from `.yoi/ticket.config.toml`.
- Profile owns durable role/system behavior.
- Launcher generates the first committed user/task message for a concrete Ticket/action.
- Dynamic content must be sent through `Method::Run`, not hidden context injection.
- Prefer a client-level API so TUI can use it without depending on `pod` internals.
- Configured `launch_prompt` refs may be exposed but not resolved if no suitable prompt-resource API exists below `pod`; do not treat them as system instruction.

Follow-up after this lands: implement `tui-ticket-role-actions` using the launcher.


---
