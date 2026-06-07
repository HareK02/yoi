<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:14:39Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T03:20:38Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready, should run before `ticket-init-role-profile-scaffold`.

Intent:
- Add an explicit runtime validation boundary for Ticket role launch config.
- Keep backend availability separate from role-launch readiness: backend-only config may list/show Tickets, but Panel Intake / Orchestrator launch must fail early if fixed role profile config is missing or non-executable.
- Preserve the policy distinction between explicit builtin selectors in config and implicit runtime fallback.

Requirements:
- Reject role launch readiness for:
  - missing role table for the target role;
  - missing role profile for the target role;
  - `profile = "inherit"` for top-level launch;
  - unresolvable concrete selector where this layer can validate it.
- Do not silently convert missing roles to `builtin:default`, `default`, or `inherit` at runtime.
- Keep backend defaults/list/show behavior working where possible.
- Panel Intake and workspace Orchestrator launch paths should use the stricter validation before spawn and emit bounded actionable diagnostics.
- Keep `TicketRoleLaunchPlan::spawn_config` `inherit` rejection as a final defensive check.

Current code map:
- `crates/ticket/src/config.rs`
  - `TicketConfig::load_workspace` returns `default_for_workspace` on missing config.
  - `RawTicketConfig::resolve` starts with `TicketRoleProfiles::default()` and overlays provided roles.
  - `TicketRoleConfig::default_for_role` uses `ProfileSelectorRef::inherit()`.
  - Tests around missing config/default roles need adjustment or expansion.
- `crates/client/src/ticket_role.rs`
  - `plan_ticket_role_launch(_with_config)` reads `config.role(context.role)` and currently does not distinguish explicit vs default role config.
  - `spawn_config` rejects `inherit` late.
- `crates/tui/src/multi_pod.rs`
  - Panel Intake / Orchestrator role launch surfaces should receive clearer diagnostics.

Implementation direction:
- Preserve enough metadata in parsed Ticket config to know whether a role was explicitly configured, or add a dedicated validation API that can report missing/implicit role defaults.
- Prefer a typed validation error with role name and actionable remediation text.
- Tests should cover backend-only config, partial role config, explicit `inherit`, and full concrete role config.

Non-goals:
- Do not implement init/scaffold generation here; `ticket-init-role-profile-scaffold` follows after this lands.
- Do not add implicit builtin fallback.
- Do not allow top-level `inherit` launch.
- Do not redesign global provider/model/profile fallback policy.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T03:20:49Z -->

## Intake summary

Implementation-ready: add strict Ticket role launch config validation before spawn, keep backend availability separate from role-launch readiness, reject missing role profile and top-level `inherit`, and avoid implicit builtin/default fallback. This should land before init/scaffold generation.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T03:20:49Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T03:21:41Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `role-launch-config-validation-coder-20260607` in worktree `.worktree/ticket-role-launch-config-strict-validation` on branch `work/ticket-role-launch-config-strict-validation`.

Scope:
- Add strict runtime validation for Ticket role launch config.
- Keep backend availability separate from role-launch readiness.
- Reject missing role config and top-level `inherit` before spawn with actionable diagnostics.
- Do not implement init/scaffold generation here; `ticket-init-role-profile-scaffold` follows after this lands.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T03:38:25Z -->

## Implementation report

## Implementation report

Coder Pod `role-launch-config-validation-coder-20260607` completed implementation in `.worktree/ticket-role-launch-config-strict-validation`.

Commit:
- `30a07b7 ticket: validate role launch config`

Reported summary:
- Added explicit launch-readiness metadata for fixed Ticket roles without breaking backend-only config loading.
- Added typed validation failures for missing role table, missing `profile`, top-level `inherit`, and profile selector resolution failure.
- Updated `plan_ticket_role_launch(_with_config)` to validate launch readiness before plan/spawn.
- Kept `TicketRoleLaunchPlan::spawn_config` defensive rejection of `inherit`.
- Added focused tests for backend-only configs, partial roles, missing profile, explicit inherit, unresolvable selectors, and successful concrete `builtin:default` launch planning.

Reported validation:
- `cargo test -p ticket config --lib`
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`

External review delegated to `role-launch-config-validation-reviewer-20260607`.

---
