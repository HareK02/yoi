<!-- event: create author: "yoi ticket" at: 2026-06-19T13:18:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-19T13:34:43Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-19T16:21:31Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、Component Model runtime path の intent、requirements、acceptance criteria、non-goals、implementation notes、validation が実装可能な粒度で揃っている。
- depends_on の `00001KV5W3PHW` minimal WASM runtime と `00001KV5W3PJ3` permission grant enforcement は closed。
- Related/context work はすべて完了または non-blocking context として確認した。
  - `00001KVFD3YSV` Plugin CLI inspection: closed。
  - `00001KVFDX9AF` HTTPS host API: closed。
  - `00001KVFDX9AY` FS host API: closed。
  - `00001KSXRQ4G8` is planning design context, not blocking relation authority。
- Prior waiting-capacity notes の blocker は解消した。現在 inprogress Ticket は 0 件、child implementation Pod はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は component-model / WIT / runtime-backend / sandbox / packaging / SDK だが、Ticket は existing raw core-Wasm packages を silently reinterpret しない、grants before Tool registration/execution/host API access、no ambient WASI fs/network/env、ordinary Tool history path、runtime kind selected by manifest metadata などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVG0HR96` body / thread / artifacts。
- `TicketRelationQuery(00001KVG0HR96)`: depends_on blockers は closed。related records は context link。
- `TicketOrchestrationPlanQuery(00001KVG0HR96)`: previous waiting notes were based on active CLI/HTTPS/FS work; all are now closed. 今回 `accepted_plan` を記録済み。
- Current workspace state:
  - Orchestrator worktree clean。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
- Code/docs context:
  - `crates/manifest/src/plugin.rs`: current runtime metadata and `yoi-plugin-wasm-1` validation。
  - `crates/pod/src/feature/plugin.rs`: current core-Wasm Plugin runtime, Tool registration/static inspection, HTTPS/FS host APIs, import validation。
  - `crates/yoi/src/plugin_cli.rs`: inspection output should report Component runtime metadata without execution。
  - Ticket body references `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md`, and Objective `00001KVG0HR9M` as design context.

IntentPacket:

Intent:
- Add explicit WebAssembly Component Model runtime support for Plugin Tool packages while preserving existing Plugin discovery, enablement, digest pinning, ToolRegistry integration, ordinary Tool history, and Plugin grant enforcement.
- Move Plugin authoring/runtime path toward WIT/canonical ABI so future `https`, `fs`, SDK, Service/Ingress APIs do not entrench the raw pointer/length core-Wasm ABI.

Binding decisions / invariants:
- Existing raw core-Wasm packages must not be silently reinterpreted as components。
- Runtime selection is manifest-driven. Component packages use explicit runtime metadata such as `kind = "wasm-component"`, component artifact path, and expected world。
- Existing raw runtime remains explicit (`kind = "wasm"`, `abi = "yoi-plugin-wasm-1"`) unless a migration/deprecation decision is recorded in this Ticket with tests updated accordingly。
- Package discovery and inspection remain read-only and must not execute components。
- Explicit enablement and digest/version/source pinning remain authoritative。
- Plugin grants are checked before Tool registration/execution and before host API calls。
- WIT imports are not authority by themselves。
- No ambient WASI filesystem/network/env is exposed。
- Component Tool registration still goes through existing ToolRegistry / model-visible schema path。
- Tool calls/results use ordinary Worker/Tool history path; no hidden context injection。
- HTTPS/FS host API security boundaries already implemented must be preserved。

Requirements / acceptance criteria:
- A package with `runtime.kind = "wasm-component"` and expected WIT world can be discovered, enabled, registered as a Tool, and executed。
- Sample Component Model Tool Plugin returns a normal Tool result through ordinary Tool path。
- Sample Plugin author source uses generated/SDK bindings rather than raw pointer/length imports/exports。
- Component Tool execution is denied without matching Plugin grants。
- Component host imports cannot bypass Plugin grant model。
- Wrong world / missing export / incompatible component fails closed with bounded diagnostic。
- Existing raw core-Wasm runtime remains explicitly supported, or a migration/deprecation decision is recorded and tests updated。
- `yoi plugin list/show` reports Component runtime metadata without executing components。
- Documentation is updated with authoring/runtime instructions and migration notes。
- Build/package impact is measured and Nix packaging/cargo hash updated if dependencies change。

Implementation latitude:
- Use `wasmtime::component` / WIT tooling or another narrow backend consistent with the codebase。
- Choose WIT names that version cleanly, e.g. `yoi:plugin/tool@1.0.0` and `yoi:host/https@1.0.0` / `yoi:host/fs@1.0.0`。
- If a staged approach is unavoidable, escalate before narrowing completion. Do not land manifest parsing alone as if it completes this Ticket。
- Keep compatibility layer and Component runtime dispatch cleanly separated。
- Use focused sample fixtures/tests rather than broad E2E process spawning。

Escalate if:
- Component runtime execution cannot be implemented without a broad architecture redesign。
- Dependency/build-size impact is large enough to need product decision。
- WIT/tool request-response typing requires a product/API decision beyond Ticket latitude。
- Preserving both raw core-Wasm and Component runtime would substantially distort implementation。
- SDK/sample generation requires external toolchain not feasible in repository validation。

Validation:
- Focused Component Plugin manifest/discovery/static inspection tests。
- Component Tool registration and execution tests。
- Grant denial before Component Tool execution / host API access。
- Wrong world / missing export / incompatible component fail-closed tests。
- Existing raw core-Wasm Plugin runtime tests remain passing or migration decision/tests updated。
- `cargo fmt --check`。
- `git diff --check`。
- relevant `cargo check` / `cargo test`。
- `nix build .#yoi` because component runtime dependencies / packaging are likely to change。

Critical risks / reviewer focus:
- WIT imports becoming implicit authority。
- Component runtime bypassing existing Plugin grant enforcement。
- Ambient WASI fs/network/env exposure。
- Component execution bypassing ordinary Tool result/history path。
- Breaking existing raw core-Wasm package behavior without explicit decision/tests。
- Inspection accidentally executing components。
- Unbounded or secret-leaking diagnostics。
- Packaging/Nix/Cargo dependency correctness and binary/build-time impact。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T16:21:50Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_component_model_runtime field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, related completed Tickets, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded Component Model runtime code context were checked. Depends-on blockers are closed, Plugin CLI / HTTPS / FS related work are closed, and no dirty-state blocker or missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---
