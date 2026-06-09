<!-- event: create author: LocalTicketBackend at: 2026-06-09T08:22:09Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T10:20:48Z -->

## Intake summary

既存 Ticket を再読し、duplicate/related work を確認した。これは既存の `pod::feature` registry、Task built-in feature、Ticket built-in feature、Ticket orchestration tools を前提に、Lua Profile / resolved Manifest から feature 単位で LLM tool surface を登録抑制する concrete implementation work item であり、新規 duplicate Ticket は不要。意図、対象 feature groups、core tools の非対象化、disabled feature は permission deny ではなく schema から消す方針、role default、受け入れ条件、検証が十分明確。blocking open question はなし。主な risk/reviewer focus は `profile/manifest`, `feature-boundary`, `authority-boundary`, `tool-schema`, `prompt-guidance`, `web-config-fail-closed`。`feature.ticket.access` / `feature.memory.access` の粒度は、既存 `TicketFeatureAccess` を再利用・拡張する範囲で実装中判断可能だが、silent broad grant になる場合は escalate する。

---

<!-- event: state_changed author: intake at: 2026-06-09T10:20:48Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake が既存 Ticket の要件を確認し、Orchestrator が routing できる状態と判断したため planning から ready に遷移します。実装はまだ開始しません。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:31:11Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T10:35:08Z -->

## Decision

Routing decision: waiting_for_core_tool_surface_stability

This Ticket remains queued for now, despite the updated parallelism preference.

Reason:
- This Ticket changes Profile/Manifest feature flags and conditional tool registration across Task/Ticket/Ticket orchestration/Pod/Memory/Web tool surfaces.
- `20260609-093050-001` is still under review and changes the core Tool execution API.
- Running both together would overlap deeply in ToolRegistry/tool surface registration and test expectations, not just create a small mechanical conflict.

State decision:
- Leave this Ticket `queued`.
- Re-read after ToolExecutionContext lands and active tool-surface branches settle.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T11:30:25Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_capacity field: state -->

## State changed

Accepted queued implementation after prior active branches landed and workspace is clean. This Ticket changes Profile/feature/tool-surface gating and no longer overlaps with an active ToolExecutionContext branch. It can proceed in parallel with Ticket schema field cleanup because expected overlap is limited to normal tool registration/test surfaces and should be manageable.

---

<!-- event: decision author: orchestrator at: 2026-06-09T11:30:25Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- The earlier blocker (`ToolExecutionContext` active core tool API migration) is resolved.
- This Ticket changes Profile feature flags and tool-surface registration; it is mostly independent from the Ticket schema field cleanup now being accepted in parallel.
- User preference is to parallelize when conflict risk is absent or expected small/manageable.

IntentPacket:

Intent:
- Control available tool surfaces through Profile feature flags / configuration so roles can expose only the intended built-in capabilities.

Binding decisions / invariants:
- Tool surface enablement should be explicit and profile/config-driven, not inferred from role names or broad defaults.
- Keep Profile authoring reusable; avoid runtime-bound data such as concrete Pod names/sockets/session state in reusable profile config.
- Do not weaken host authority, scope, network, Ticket backend, or memory boundaries; feature flags may expose tools only when underlying authority/config allows them.
- Avoid adding compatibility aliases for old ad-hoc behavior unless a concrete current requirement exists.
- Preserve clear failure when a requested feature/tool surface lacks required configuration.
- Keep changes bounded to profile/manifest/tool registration/guidance/tests; do not redesign the plugin system.

Reviewer focus:
- Verify features gate tool registration without bypassing authority checks.
- Verify default/project role profiles expose intended surfaces and no unintended tools.
- Verify missing/invalid feature config fails clearly.
- Verify docs/prompts/guidance match the new feature flag model.

Validation:
- Focused profile/manifest/tool registry tests.
- Built-in tool surface/capability tests for affected Task/Ticket/Pod/Memory/Web tools.
- `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `cargo check --workspace`, `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T11:31:52Z -->

## Implementation report

Implementation routing started in parallel with the Ticket action/attention field cleanup.

Worktree/branch:
- Worktree: `.worktree/profile-feature-flags-tool-surface`
- Branch: `profile-feature-flags-tool-surface`
- Base/routing commit: `41133e0 ticket: route feature flags and attention cleanup`

Spawned sibling Coder Pod:
- `coder-profile-feature-flags-tool-surface`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Parallel-routing rationale:
- ToolExecutionContext, TicketList, Panel, and session analytics work have landed.
- This Ticket is mostly Profile/feature/tool-surface registration.
- It is expected to have only small/manageable overlap with the Ticket schema field cleanup running in parallel.

---

<!-- event: implementation_report author: coder at: 2026-06-09T12:05:22Z -->

## Implementation report

Implemented Profile feature flags for built-in tool-surface registration.

Summary:
- Added resolved `feature` config with explicit flags for `task`, `memory`, `web`, `pod_management`, `ticket`, and `ticket_orchestration` plus Ticket `access = "read_only" | "lifecycle"`.
- Changed controller registration so optional built-in surfaces are registered only when enabled by Profile/config; core filesystem/process tools remain static and scope-gated.
- Kept safety boundaries: web tools still fail closed under web config, memory feature requires `[memory]`, pod-management feature requires non-empty delegation scope, Ticket tools still use backend host authority.
- Split Ticket base and orchestration tool declarations so Ticket orchestration can be surfaced independently.
- Migrated project role profiles and builtin default profile to explicit feature settings.

Validation passed:
- `cargo test -p manifest feature --lib`
- `cargo test -p pod feature --tests`
- `cargo test -p pod pod_management_feature_requires_delegation_scope --test controller_test`
- `cargo test -p tools --test integration`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T12:10:16Z -->

## Implementation report

Reviewer routed after coder implementation commit `f0f6cc9`.

Reviewer Pod:
- `reviewer-profile-feature-flags-tool-surface`
- Worktree: `.worktree/profile-feature-flags-tool-surface`

Review focus includes explicit bool feature authority, concrete resolved Manifest feature values, disabled feature tool omission from schema, role profile surfaces, authority/fail-closed preservation, and the `feature.pods` vs implementation-reported `feature.pod_management` naming mismatch.

---

<!-- event: review author: reviewer-profile-feature-flags-tool-surface at: 2026-06-09T12:18:57Z status: request_changes -->

## Review: request changes

Review result: request_changes.

Findings:

1. Coder role still enables Task tools by default. Ticket/review focus requires the Coder profile not to expose Ticket management, Ticket orchestration, Task, or Pod orchestration by default. The implementation correctly disables Ticket, Ticket orchestration, and Pod management in `.yoi/profiles/coder.lua`, but leaves `feature.task.enabled = true`, and controller registration registers Task tools whenever that flag is enabled. This means the Coder tool schema still exposes TaskCreate/TaskUpdate/etc.
   - `.yoi/profiles/coder.lua:8-14`
   - `crates/pod/src/controller.rs:545-548`

2. The user-facing Pod feature flag name does not match the Ticket spec. The Ticket names the initial group `feature.pods`, but the implementation exposes only `feature.pod_management`. Since manifest/Profile config currently ignores unknown subfields rather than rejecting or aliasing them, a profile following the Ticket text with `feature.pods.enabled = true` would not enable Pod-management tools, and would likely fail only indirectly at missing-tool time. Either align the name with the Ticket or update the accepted spec and reject/diagnose the old spelling explicitly.
   - `.yoi/tickets/20260609-082209-001/item.md:31-37`, `.yoi/tickets/20260609-082209-001/item.md:56-59`
   - `crates/manifest/src/config.rs:77-84`
   - `crates/manifest/src/lib.rs:95-101`

Validation run:

- `cargo fmt --check` passed
- `git diff --check HEAD^..HEAD` passed
- `cargo run -q -p yoi -- ticket doctor` passed
- `cargo check --workspace` passed
- `cargo test -p manifest feature --lib` passed
- `cargo test -p pod feature --tests` passed
- `cargo test -p pod pod_management_feature_requires_delegation_scope --test controller_test` passed
- `cargo test -p tools --test integration` passed

Positive checks: disabled optional surfaces are omitted from registry by `controller.rs` gating rather than merely denied at execution; resolved feature structs use concrete booleans; Ticket base and orchestration registration are separable; web registration is omitted when `feature.web.enabled=false` and still uses existing provider/config fail-closed checks when enabled; enabling Ticket/Pod management still goes through host-authority/delegation checks.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T12:22:09Z -->

## Implementation report

Reviewer requested changes on the initial implementation. Fix applied on branch `profile-feature-flags-tool-surface`:

- Renamed the user-facing profile feature from `feature.pod_management` to the Ticket-specified `feature.pods`.
- Kept internal prompt helper naming unchanged where it describes derived tool capabilities rather than profile syntax.
- Set project Coder profile `feature.task.enabled = false` so Coder no longer exposes Task tools by default.

Fix commit: `2fd37af fix: align pod feature flag naming`

Validation after fix:
- `cargo fmt`
- `cargo test -p manifest feature --lib`
- `cargo test -p pod feature --tests`
- `cargo test -p pod pods_feature_requires_delegation_scope --test controller_test`
- `cargo test -p tools --test integration`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `cargo fmt --check`

---

<!-- event: review author: reviewer-profile-feature-flags-tool-surface-r2 at: 2026-06-09T12:27:05Z status: request_changes -->

## Review: request changes

Re-review result: request_changes.

Prior findings status:
- Fixed: user-facing Profile/Manifest syntax is now `feature.pods`; controller error/test/profile resources use `[feature.pods]`, and remaining `pod_management` references are internal prompt capability helper names rather than profile syntax.
- Fixed for Coder: `.yoi/profiles/coder.lua` now sets `feature.task.enabled = false` and keeps Ticket/Ticket orchestration/Pods disabled.

Blocking finding:

1. Several project role profiles still expose Task tools by default despite the Ticket role defaults saying these roles should disable Task. `register_pod_tools` registers TaskCreate/TaskUpdate/TaskGet/TaskList whenever `feature_config.task.enabled` is true, so these profile defaults are model-visible tool-surface grants, not inert metadata.
   - `.yoi/profiles/intake.lua:7-13` has `task = { enabled = true }`, but the Ticket says Intake `task = disabled`.
   - `.yoi/profiles/reviewer.lua:7-13` has `task = { enabled = true }`, but the Ticket says Reviewer `task = disabled`.
   - `.yoi/profiles/companion.lua:7-13` has `task = { enabled = true }`, but the Ticket says Companion `task = disabled`.
   - `.yoi/profiles/orchestrator.lua:8-14` has `task = { enabled = true }`; the Ticket says Orchestrator task should be disabled unless explicitly justified. I do not see a recorded justification in the implementation/report.
   - `.yoi/profiles/_base.lua:32-38` and `resources/profiles/default.lua:29-35` also keep Task enabled by default; this may be fine for the builtin/default coding profile if intentional, but the project role profiles above need to match the Ticket or document an accepted deviation.
   - Registration path: `crates/pod/src/controller.rs:545-548`.

Secondary gap:
- The acceptance condition asks for tests covering tool definition lists for Orchestrator, Coder, Intake, Reviewer, and Companion profiles. The current focused tests cover synthetic feature manifests and the Coder profile was fixed by file inspection, but I did not find role-profile tool-list tests that would catch the Intake/Reviewer/Companion Task exposure.

Validation run:
- `cargo test -p manifest feature --lib` passed (4 tests).
- `cargo test -p pod feature --tests` passed (45 lib tests plus 3 controller feature tests; other filtered test binaries had 0 matching tests).
- `cargo test -p pod pods_feature_requires_delegation_scope --test controller_test` passed (1 test).
- `cargo test -p tools --test integration` passed (14 tests).
- `cargo fmt --check` passed.
- `git diff --check` passed.
- `cargo run -q -p yoi -- ticket doctor` passed (`doctor: ok`).
- `git status --short` is clean.

Merge readiness: not merge-ready until the remaining project role defaults and role-profile tool-list test coverage are aligned with the Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T12:32:00Z -->

## Implementation report

Second review requested one remaining change set:

- Project role profiles beyond Coder (`intake`, `reviewer`, `companion`, and `orchestrator`) still inherited/used `feature.task.enabled = true`, contrary to the Ticket's explicit role defaults.
- There was no regression test that asserted role-like tool surfaces across Orchestrator/Coder/Intake/Reviewer/Companion.

Fix applied on branch `profile-feature-flags-tool-surface`:

- Set `feature.task.enabled = false` in the remaining project role profiles.
- Added controller regression test `project_role_tool_surfaces_keep_task_disabled_and_pods_role_scoped`, covering the five role-like tool surfaces for Task tool absence and Pod tool exposure only where intended.

Fix commit: `507863f fix: lock project role feature surfaces`

Validation after fix:
- `cargo fmt`
- `cargo test -p pod project_role_tool_surfaces_keep_task_disabled_and_pods_role_scoped --test controller_test`
- `cargo test -p pod pods_feature_requires_delegation_scope --test controller_test`
- `cargo test -p manifest feature --lib`
- `cargo test -p pod feature --tests`
- `cargo test -p tools --test integration`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`

---
