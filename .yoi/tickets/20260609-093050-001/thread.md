<!-- event: create author: LocalTicketBackend at: 2026-06-09T09:30:50Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T09:55:00Z -->

## Intake summary

既存 Ticket を確認し、同目的の未完了重複は見当たりません。要件は Tool 実行 API に `ToolExecutionContext` を導入し、`call_id` / response-local `batch_id` / `call_index` を Tool 実装へ渡すことです。Worker は従来どおり approved tool calls を並列実行し、resource scheduling policy は持たないこと、old Tool API 互換層を長期維持しないこと、Interceptor / hook に lock lifecycle を持たせないことが binding decision です。実装側には `batch_id` 生成方式や session log への記録有無の裁量があります。関連し得る Ticket として `20260609-085148-001` は response 単位の batching analytics であり、目的が異なるため重複ではありません。リスク/レビュー焦点は public API 変更、Worker tool execution path、Interceptor/hook 意味論、並列実行維持、既存 built-in tools の一括移行です。受け入れ条件と validation が明確なため implementation_ready と判断します。

---

<!-- event: state_changed author: intake at: 2026-06-09T09:55:00Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件・非目標・受け入れ条件・レビュー焦点が routing 可能な状態で確認されたため、planning から ready にします。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:01:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-09T10:03:41Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Accepted queued implementation after reading the Ticket and current workspace state. This is a core tool execution API migration and should land before changing individual tool outputs such as TicketList. No active implementation worktree exists, and the workspace record updates were committed before acceptance.

---

<!-- event: decision author: orchestrator at: 2026-06-09T10:03:41Z -->

## Decision

Routing decision: implementation_ready

Evidence checked:
- Ticket body and Intake summary.
- Current queued set: `20260609-093050-001`, `20260609-085212-001`, and `20260609-084725-001`.
- Current workspace/worktree state after committing queued record updates: main workspace clean, no active implementation worktree.
- Relation/order context: `TicketList` output slimming should wait for this core tool execution API migration.

Reason:
- The Ticket has clear implementation requirements: introduce `ToolExecutionContext`, pass it through Tool execution, and move context-dependent path/working-directory/backend access out of static Tool definitions where appropriate.
- It is a lower-level API migration that should land before individual tool output/UX changes.
- No concrete missing decision remains requiring return to planning.

IntentPacket:

Intent:
- Pass a `ToolExecutionContext` into Tool execution so tool behavior can depend on runtime context without encoding runtime-dependent data in static tool definitions.

Binding decisions / invariants:
- Static Tool definitions/schemas remain static and reusable.
- Runtime-dependent values such as cwd/workspace context/backend handles should flow through execution context where appropriate, not through schema metadata or global mutable state.
- Existing tool behavior must remain compatible unless a tool explicitly needs the new context.
- Keep capability/authority checks explicit and typed; do not use `ToolExecutionContext` as an unstructured escape hatch.
- Do not weaken scope, host-authority, Ticket backend, or web/network safety boundaries.
- Avoid broad speculative refactors beyond the execution-path migration required for this Ticket.
- Update built-in tools/tests that need context, especially file/path/Ticket/backend-sensitive surfaces.
- Preserve LLM-facing tool names and schemas unless a specific schema change is required.

Implementation latitude:
- Coder may choose exact `ToolExecutionContext` fields and lifetimes if they keep ownership/thread-safety practical and avoid leaking runtime/session-private data into static schemas.
- Coder may introduce default/empty context helpers for tests and tools that do not need context.
- If a subset of tools must be migrated first, keep a coherent buildable boundary and report remaining follow-up explicitly.

Escalate if:
- The change requires redesigning the whole ToolRegistry/plugin system.
- Context fields would need to carry raw Worker/history/session mutable access.
- Authority checks cannot remain explicit and typed.
- Too many unrelated tool schema/output changes become necessary.

Validation:
- Focused Tool trait/ToolRegistry tests for context passing.
- Focused tests for context-sensitive built-in tools such as Ticket/backend/path tools if affected.
- Existing tool schema/name tests.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- `cargo check --workspace`.
- `nix build .#yoi`.

---
