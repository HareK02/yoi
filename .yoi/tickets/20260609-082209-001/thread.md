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
