<!-- event: create author: "yoi ticket" at: 2026-07-15T16:18:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-15T16:18:40Z -->

## Plan

Background:
- `.yoi/ticket.config.toml` currently owns Ticket backend root/provider, Ticket record language, orchestration defaults, and fixed role launch config.
- Workspace backend / Runtime / Worker Ticket tools are moving toward Workspace-owned Ticket authority, so a separate Ticket-specific config file is no longer the right durable settings boundary.
- Local backend operational settings such as listen address, DB path, runtime store, and remote runtime endpoint should remain local/backend settings, not project Ticket policy.

Requirements:
- Introduce or reuse a tracked Workspace project settings file as the durable authority for Ticket configuration.
- Move Ticket backend provider/root, Ticket record language, orchestration defaults, and fixed role launch config under a Workspace settings namespace.
- Keep local-only backend/runtime settings separate from tracked project/workspace Ticket policy.
- Update Ticket CLI, Worker Ticket feature setup, workspace-server Ticket backend endpoint, Panel/role launch code, and tests to resolve Ticket settings from the Workspace settings authority.
- Preserve a narrow read-only migration path from `.yoi/ticket.config.toml` only if needed for transition, but do not keep it as the long-term active authority.
- Ensure workspace-server Ticket backend endpoint honors the configured Ticket backend root instead of hard-coding `.yoi/tickets`.

Acceptance criteria:
- New/updated tests cover Workspace settings loading for Ticket backend root, language, orchestration defaults, and role slots.
- Existing Ticket CLI and Worker Ticket tools continue to work against the same `.yoi/tickets` data by default.
- A non-default Ticket backend root configured in Workspace settings is respected by both direct CLI/local paths and workspace-server-backed Worker Ticket tools.
- Documentation or in-code migration notes state that `.yoi/ticket.config.toml` is obsolete/replaced.
- `cargo test` for affected crates and `nix build .#yoi` pass.


---

<!-- event: intake_summary author: hare at: 2026-07-15T16:18:40Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-15T16:18:40Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
