<!-- event: create author: "yoi ticket" at: 2026-06-24T19:31:28Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:39:33Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:39:33Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T19:39:36Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T19:40:42Z -->

## Decision

Routing decision: return_to_planning

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は Plugin WASM runtime の全面 redesign を要求しているが、acceptance criteria が legacy runtime removal、Component Model only 化、Tool/Service/Ingress runtime、host-managed WebSocket driver、event queue/backpressure/retry、output command/grant checks、Manifest/WIT/PDK/templates 更新、status diagnostics、docs/tests/Nix validation まで含んでおり、単一の実装 Ticket としては broad multi-surface effort になっている。
- bounded context check で既存 `docs/design/plugin-component-model.md` と `docs/development/plugin-development.md` を確認したところ、現行 docs/code direction は raw core-Wasm runtime を explicit compatibility/transitional support として残し、`PluginInstanceRegistry` で legacy raw-wasm / `yoi:plugin/tool@1.0.0` component packages を adapter する方針を記録している。これは本 Ticket の「compatibility alias / fallback を作らない」「legacy runtime を active runtime から削除」と衝突する可能性がある。
- `TicketRelationQuery` は blocking relation 0 件、`TicketOrchestrationPlanQuery` は既存 plan 0 件だったが、関連 docs/code の recorded direction と Ticket 要件の間に未解決の design decision が残っている。
- これは risk flag だけの問題ではなく、実装前に release/pre-release compatibility 方針、WIT/manifest/service event API、WebSocket driver execution model、legacy removal scope、分割順序を固定しないと coder が project-wide API を決めてしまう種類の不足である。

Evidence checked:
- Ticket body / thread: `00001KVXHVCR5` の `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで、詳細な design decision / split plan / implementation order は記録されていない。
- Relations / orchestration plan: relation 0 件、plan 0 件。
- Docs/code context:
  - `docs/design/plugin-component-model.md`: Component Model migration、legacy raw `wasm` explicit/transitional support、`PluginInstanceRegistry` adapter 方針、host API grant boundary。
  - `docs/development/plugin-development.md`: `wasm-component` preferred runtime だが raw core-Wasm runtime が compatibility/transitional support として説明されている。
  - Grep context: `LegacyToolAdapter`, `wasm-component`, `host_api.websocket`, Service/Ingress/PluginInstance 周辺が複数 file に分散。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、inprogress Ticket は 0 件、implementation worktree/branch for this Ticket は未作成。

Missing decision / information:
- この Ticket を「一括実装 Ticket」として進めるのか、Objective + concrete Tickets に分割するのか。
- Legacy raw `wasm` / `yoi-plugin-wasm-1` を即削除するか、現行 docs の transitional/adapter 方針を明示的に撤回するか。
- v0 で実装する exact surface: Tool runtime cleanup のみ、Service/Ingress event queue boundary、WebSocket driver、WIT/PDK/templates、diagnostics/status のどこまでを同一 Ticket の done condition に含めるか。
- `plugin.toml` / WIT / PDK の concrete schema/API shape と、現行 `host_api.websocket` pull API を reject/remove/deprecate/internal-only のどれにするか。
- Wasmtime/component/service runtime の binary size / Nix / sandbox policy / execution limits に関する validation scope。

Why implementation latitude is insufficient:
- Coder の local tactic selection で legacy compatibility policy や public/pre-release Plugin API direction を固定すると、manifest/WIT/PDK/templates/docs/tests すべての public-facing shape を一度に決めてしまう。
- Service/Ingress event runtime と WebSocket driver は execution model と host authority boundary の設計そのものであり、単なる file-local implementation detail ではない。
- 既存 docs に recorded direction があり、Ticket がそれを上書きするかどうかを実装者判断にすると project design authority を越える。

Next planning question/action:
- この Ticket を Objective/roadmap context として扱い、以下のような concrete Tickets に分割するかを決める。
  1. Legacy raw `wasm` runtime removal / manifest rejection / docs fixture cleanup。
  2. Service/Ingress event queue and PluginInstance lifecycle boundary。
  3. Host-owned WebSocket driver design/implementation。
  4. WIT/PDK/templates update for event/command model。
  5. Plugin status/diagnostics overview。
- あるいは、まず design/planning artifact Ticket に変更し、exact v0 schema/API/non-goals を決めてから実装 Ticket を切る。

Next action:
- `queued -> planning` に戻し、Intake/planning で split plan または explicit v0 scope decision を作る。
- Coder/Reviewer Pod は起動しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T19:40:53Z from: queued to: planning reason: missing_plugin_runtime_design_scope_and_split_decision field: state -->

## State changed

Queued Ticket is returned to planning before any implementation side effect.

Concrete missing decisions/information:
- Whether this broad Plugin runtime redesign is a single implementation Ticket or should become Objective context plus concrete implementation Tickets.
- Whether current recorded transitional/adapter support for raw `wasm` / `yoi-plugin-wasm-1` is explicitly revoked before public release.
- Exact v0 scope for legacy removal, Service/Ingress event queue, WebSocket driver, WIT/PDK/templates, and diagnostics/status.
- Concrete manifest/WIT/PDK/API shape and how current `host_api.websocket` pull API is handled.
- Validation and packaging scope for component/service runtime changes.

Checked context:
- Ticket body/thread; relation 0 件; orchestration plan 0 件。
- `docs/design/plugin-component-model.md` and `docs/development/plugin-development.md`, which currently describe raw core-Wasm as explicit compatibility/transitional support and mention adapter behavior.
- Workspace state is clean and no implementation worktree/branch was created.

Why implementation latitude is insufficient:
- These choices fix project-wide Plugin API, manifest, WIT, PDK, runtime authority, and service execution semantics. They conflict with or supersede existing recorded design direction and are not safe as coder-local tactic choices.

Next planning action:
- Decide split plan / Objective context or convert this to an explicit design/planning artifact Ticket with a bounded v0 API decision before implementation routing.

---
