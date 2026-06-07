<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:14:39Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T03:43:33Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready after `ticket-role-launch-config-strict-validation` landed.

Intent:
- Add a Ticket-focused scaffold/init path that writes an explicit `.yoi/ticket.config.toml` with concrete fixed role profiles.
- This is explicit config generation, not runtime fallback. The generated file may choose builtin presets, but the runtime must continue requiring explicit role config.

Current state:
- There is no top-level `yoi init` command in `crates/yoi/src/main.rs`.
- `yoi ticket` currently has no `init` command.
- `yoi ticket` already documents fallback backend behavior when config is absent, but role launch validation now requires explicit role config for Panel Intake/Orchestrator launches.

Implementation direction:
- Introduce a narrow `yoi ticket init` scaffold command rather than a broad product-level init redesign.
- The command should create `.yoi/ticket.config.toml` if missing, and optionally ensure the local Ticket root `.yoi/tickets` exists.
- Default generated config should explicitly include:
  - `[backend] provider = "builtin:yoi_local"`, `root = ".yoi/tickets"`;
  - fixed `[roles.*]` tables for intake, orchestrator, coder, reviewer, investigator;
  - each role has a concrete `profile`, initially `builtin:default` unless role-specific builtin profiles are introduced in the same small diff;
  - each role has explicit `workflow` matching the role defaults.
- Do not overwrite an existing `.yoi/ticket.config.toml` unless an explicit force flag is intentionally added and tested. A minimal first pass can fail with an actionable diagnostic when the config already exists.
- Generated config should pass the strict role launch validation added by the previous ticket.

Suggested command shape:
- `yoi ticket init`
- Optional flags only if small and useful:
  - `--force` is not required for first pass;
  - `--print`/dry-run is not required for first pass.

Code map:
- `crates/yoi/src/ticket_cli.rs`: add parse/run/help for `init` and tests.
- `crates/ticket/src/config.rs`: add reusable scaffold string/helper if that keeps config generation testable near config types.
- `crates/client/src/ticket_role.rs`: use existing launch validation tests to verify generated config is launch-ready.
- `.yoi/ticket.config.toml`: update this repository's config through the new intended shape or keep repository config update in main workspace if child worktree excludes `.yoi`; the implementation should at least make the scaffold generate that shape.

Non-goals:
- Do not loosen runtime validation.
- Do not add implicit builtin/default fallback.
- Do not implement broad global `yoi init` unless the existing CLI structure makes it clearly smaller than `yoi ticket init`.
- Do not design role-specific builtin profiles unless it is trivial and scoped; `builtin:default` as explicit scaffold output is acceptable for now.

Validation:
- Tests should cover generated config shape, refusal to overwrite existing config, and successful role launch validation against generated config.
- Run focused `cargo test -p ticket config --lib`, `cargo test -p client ticket_role --lib`, and `cargo test -p yoi ticket` or equivalent ticket CLI tests.
- Run `cargo fmt --check` and `git diff --check`.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T03:43:38Z -->

## Intake summary

Implementation-ready after strict role launch validation: add a narrow `yoi ticket init` scaffold path that writes explicit backend and fixed role profile config to `.yoi/ticket.config.toml`, using concrete selectors such as explicit `builtin:default` for now, without adding runtime fallback.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T03:43:38Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T03:44:16Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `ticket-init-scaffold-coder-20260607` in worktree `.worktree/ticket-init-role-profile-scaffold` on branch `work/ticket-init-role-profile-scaffold`.

Scope:
- Add a narrow `yoi ticket init` scaffold command.
- Generate explicit backend and fixed role profile/workflow config in `.yoi/ticket.config.toml`.
- Use concrete role profiles in the generated config; explicit `builtin:default` is acceptable for the first pass.
- Do not loosen strict role launch validation or add runtime fallback.
- Do not overwrite existing config in the first pass.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T03:58:12Z -->

## Implementation report

## Implementation report

Coder Pod `ticket-init-scaffold-coder-20260607` completed implementation in `.worktree/ticket-init-role-profile-scaffold`.

Commit:
- `f265098 ticket: add ticket config init scaffold`

Reported summary:
- Added narrow `yoi ticket init`.
- It creates `.yoi/ticket.config.toml` only when missing and ensures `.yoi/tickets` exists.
- Existing `.yoi/ticket.config.toml` is not overwritten and fails with an actionable diagnostic.
- Added reusable `ticket_config_scaffold()` in `crates/ticket/src/config.rs`.
- Generated config includes backend provider/root plus fixed roles intake/orchestrator/coder/reviewer/investigator.
- Each generated role uses explicit `profile = "builtin:default"` and explicit default `workflow`.
- Added tests for config shape, CLI init/no-overwrite behavior, CLI help, and launch planning for Intake/Orchestrator from the scaffold.

Reported validation:
- `cargo test -p ticket config --lib`
- `cargo test -p client ticket_role --lib`
- `cargo test -p yoi ticket`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`

External review will be delegated before merge.

---
