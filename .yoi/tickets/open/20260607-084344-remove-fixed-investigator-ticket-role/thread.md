<!-- event: create author: LocalTicketBackend at: 2026-06-07T08:43:44Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: intake at: 2026-06-08T11:06:21Z -->

## Plan

## Intake refinement

Readiness: implementation_ready.

Binding decisions / invariants:

- `investigator` is no longer a fixed Ticket role or configured role slot. The fixed Ticket roles are `intake`, `orchestrator`, `coder`, and `reviewer`.
- Investigation remains available only as task-specific helper Pod work launched by Intake/Orchestrator/planning when useful; it must not require a `[roles.investigator]` config entry or a `project:investigator` profile slot.
- Existing/current `[roles.investigator]` config must not be silently used as a supported role. The implementation should choose a clear unsupported-config diagnostic or documented migration behavior and keep that behavior test-covered.
- Do not reintroduce the removed TUI `:ticket investigate` command or add a generic arbitrary role registry in this cleanup.

Implementation latitude:

- The implementation may choose the smallest cohesive code shape for removing the enum/config/default/scaffold/test references, including whether compatibility is expressed as an explicit config error or migration note.
- Local code mapping and exact test updates are implementation details as long as role launch remains working for intake/orchestrator/coder/reviewer.

Reviewer / Orchestrator focus:

- Confirm all fixed-role surfaces are removed, not only user-facing docs.
- Confirm helper-Pod investigation wording remains possible without implying a configured Ticket role.
- Confirm role config failure behavior for stale `[roles.investigator]` is intentional and covered.

Risk flags: ticket-config, role-boundary, docs, compatibility.

Validation:

- Focused tests for `ticket::config`, `client::ticket_role`, and affected TUI/panel role-launch paths.
- `target/debug/yoi ticket doctor`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi` if code/package/runtime resources are changed.

---

<!-- event: intake_summary author: intake at: 2026-06-08T11:06:46Z -->

## Intake summary

既存 Ticket を確認し、`investigator` を固定 Ticket role/config slot から削除する cleanup として要件を同期した。固定 role は `intake` / `orchestrator` / `coder` / `reviewer` のみとし、調査は必要時の task-specific helper Pod として残す。stale `[roles.investigator]` は silently supported にせず、明確な診断または migration として扱う。Orchestrator は実装 routing 可能。risk_flags: ticket-config, role-boundary, docs, compatibility。

---

<!-- event: state_changed author: intake at: 2026-06-08T11:06:46Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket planning が完了しました。workflow_state planning -> ready。


---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T11:15:16Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-08T11:16:48Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body and thread record the product decision: fixed Ticket role slots are Intake / Orchestrator / Coder / Reviewer, and Investigator is not an official fixed role.
- The requested behavior is bounded and observable: remove Investigator from role enum/config/scaffold/prompt/workflow surfaces while preserving ad-hoc read-only investigation as an ordinary Pod/workflow pattern where needed.
- Remaining uncertainty is code cleanup/test coverage, not a design-boundary question.

Evidence checked:
- Ticket body and thread, including queued event.
- Workspace state: no matching branch/worktree exists; main workspace is clean before routing.
- Code/prompt search for `Investigator`, `investigator`, `roles.investigator`, `project:investigator`, and `ticket investigate` across Ticket role config, client launch, TUI/Panel, profiles, prompts, workflows, and docs.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Remove the fixed Investigator Ticket role from the product/config/runtime surfaces and keep only the fixed Ticket role slots that are actually supported.

Binding decisions / invariants:
- Fixed Ticket roles are Intake, Orchestrator, Coder, and Reviewer.
- Investigator is not a fixed Ticket role and should not appear in required role config/scaffold/enum/action surfaces.
- Ad-hoc investigation remains possible through normal Pods/workflows when a human/Orchestrator explicitly asks for read-only investigation, but it is not a fixed Ticket role slot.
- Do not remove generic Pod spawning, peer messaging, or read-only review/investigation capability.
- Do not change Ticket workflow-state semantics or the planning/ready/queued/inprogress/done model.
- Avoid compatibility aliases that silently keep `investigator` as an official fixed role unless needed only to reject legacy config with a clear diagnostic.

Requirements / acceptance criteria:
- Remove Investigator from fixed Ticket role enum/config parsing/scaffold/defaults.
- Remove project/local profile references for `project:investigator` or equivalent official role config if present and unused.
- Remove Panel/TUI/CLI/workflow references that present Investigator as a first-class Ticket role/action.
- Update role-launch prompt/config tests to expect only Intake/Orchestrator/Coder/Reviewer fixed slots.
- Keep any generic “read-only investigation” language as a task/workflow tactic rather than a fixed role.
- Add/update tests so configs requiring only the four fixed roles pass, and legacy/unknown investigator role config is rejected or ignored according to the explicit parser policy.

Implementation latitude:
- Coder may leave historical Ticket records untouched.
- Coder may retain lowercase text such as “investigation” in generic prose if it is not an official role name/action/config surface.
- Coder may choose whether legacy `roles.investigator` is rejected as unknown or treated as ignored-with-diagnostic if existing config parser has such a policy; report the decision.
- Coder may update docs/workflows narrowly where they refer to fixed roles.

Escalate if:
- Removing Investigator requires a broader generic role registry redesign.
- Existing role config parser cannot reject/ignore extra role tables without changing config compatibility policy.
- Panel/CLI surfaces depend on Investigator for a still-supported action that needs a replacement design.

Validation:
- Focused client/ticket-role config tests.
- TUI/Panel tests if action lists or role displays change.
- Profile/scaffold tests for `.yoi/ticket.config.toml` generation.
- Search maintained code/prompts/workflows/docs for official Investigator role references.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because role config/profile/prompt/workflow surfaces may change, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/client/src/ticket_role.rs`: fixed role enum/config/launcher/prompt handling.
- `crates/tui/src/multi_pod.rs` / `workspace_panel.rs`: Panel action/role launch surfaces.
- `crates/yoi/src/ticket_cli.rs`: ticket init/scaffold/config tests.
- `.yoi/profiles` and profile registry/resources for role profile selectors.
- `.yoi/workflow/*.md`, `docs/development/*.md`, and `resources/prompts` for fixed-role wording.

Critical risks / reviewer focus:
- Do not remove Intake role just because workflow state no longer uses `intake` as a state.
- Ensure Orchestrator/Coder/Reviewer role launches still work.
- Ensure scaffolded Ticket config remains executable and concrete.
- Ensure no maintained prompt/workflow still implies an official Investigator role slot.
- Avoid broad docs churn outside role-surface cleanup.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T11:16:58Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and fixed-role code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---
