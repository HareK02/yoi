---
title: 'Plugin Service/Ingress component lifecycle surface'
state: 'inprogress'
created_at: '2026-06-20T13:01:37Z'
updated_at: '2026-06-20T14:50:21Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-20T13:28:19Z'
---

## 背景

現行の外部 Plugin 実装は Tool surface のみを提供しており、実行モデルも Tool call ごとの artifact 実行に寄っている。Discord Bridge や Minecraft Plugin 的な拡張では、Tool / Service / Ingress が同じ Plugin instance とその内部構造・状態・設定・diagnostics を共有できる必要がある。

この Ticket では、Plugin を「Tool call のたびに実行される wasm artifact」ではなく、Pod lifetime に紐づいて host が管理する **Plugin instance** として再設計し、一気に実装する。mutable state を持たない Plugin も意味論上は instance として扱い、per-call instantiate / pooling / lightweight execution は host runtime の最適化または legacy adapter の実装詳細に留める。

既存前提:

- `00001KT6Q08R9` で Feature contribution registry は Tool / Hook / BackgroundTask / Service provider descriptor 境界を導入済み。
- 現行 Plugin package/runtime は Tool registration/execution まで実装済み。
- `PluginSurface` には Tool / Service / Ingress などの概念があるが、実際の external wasm Plugin runtime は Tool only。
- Plugin output は hidden context injection ではなく、Tool result、committed history、explicit notification/history path へ流す原則を維持する。

## Core model

- Plugin package は enabled/digest/grant 検証後、Pod 内に 1 個以上の host-managed `PluginInstance` として登録される。
- Tool / Service / Ingress は Plugin instance が提供する surface であり、独立した実行主体ではない。
- Tool call は model-visible ToolRegistry から thin dispatch handle を通り、同じ Plugin instance の command/query handler に入る。
- Ingress event は host-managed external/internal event source から bounded typed event として入り、同じ Plugin instance の event handler に入る。
- Service は Plugin instance lifecycle / background-owned capability / status/diagnostics を表す surface であり、Tool の代替ではない。
- Wasm component は Plugin instance interface を実装する。Tool-only Plugin も instance interface に乗せる。
- 既存の Tool-only component/raw wasm runtime は互換 adapter として維持し、長期意味論は instance-oriented に揃える。

概念図:

```text
Enabled Plugin Package
  -> PluginInstanceRegistry
     -> PluginInstance
        -> lifecycle: start/status/stop
        -> tool dispatch: handle_tool(tool_name, input_json)
        -> ingress dispatch: handle_ingress(event_kind, event_json)
        -> diagnostics/status/actions via host-mediated paths

ToolRegistry entry
  -> PluginInstanceTool { plugin_ref, tool_name, registry }
  -> registry.call_tool(plugin_ref, tool_name, input)

Ingress source
  -> registry.deliver_ingress(plugin_ref, ingress_name, event)
```

## Wasm / PDK interface direction

Define a new Component world/interface, e.g. `yoi:plugin/instance@1.0.0`, separate from the current Tool-only world. Exact WIT/schema names can change during implementation, but the implemented interface must represent the following lifecycle:

- `start(config_json) -> result/status`
- `handle_tool(tool_name, input_json) -> tool_result_json`
- `handle_ingress(ingress_name, event_json) -> action_batch_json/status`
- `status() -> status_json`
- `stop(reason_json) -> result/status`

PDK shape should be instance-oriented, for example:

```rust
struct MyPlugin {
    // Mutable state is allowed but not required.
}

impl yoi_plugin_pdk::Plugin for MyPlugin {
    fn start(ctx: StartContext) -> Result<Self, Error>;
    fn handle_tool(&mut self, ctx: ToolContext, tool: &str, input: JsonValue) -> Result<ToolOutput, Error>;
    fn handle_ingress(&mut self, ctx: IngressContext, ingress: &str, event: JsonValue) -> Result<ActionBatch, Error>;
    fn status(&self, ctx: StatusContext) -> Result<PluginStatus, Error>;
    fn stop(self, ctx: StopContext) -> Result<(), Error>;
}

yoi_plugin_pdk::export_plugin_instance!(MyPlugin);
```

Tool-only helper macros may remain, but should become compatibility/convenience wrappers over the instance model where practical.

## Host runtime design

Add a `PluginInstanceRegistry` in the Pod feature/plugin layer.

Responsibilities:

- Build instance descriptors from discovered + enabled Plugin records.
- Validate digest/version/source/grants before instance registration.
- Start instances during Pod/Worker setup at the point where Tool registration can receive registry handles.
- Register Tool surface entries as dispatch handles, not self-contained artifact executors.
- Route Tool calls to `PluginInstance::handle_tool`.
- Route Ingress events to `PluginInstance::handle_ingress`.
- Provide status/diagnostics for `yoi plugin list/show` or equivalent inspection.
- Stop instances on Pod shutdown/runtime teardown.
- Keep model-visible Tool schemas run-stable for a Worker run; instance state must not mutate Tool schema mid-run.

Instance execution should be serialized per Plugin instance initially:

- Each instance owns its Wasm `Store`/component instance or legacy adapter state.
- Calls enter through an actor/queue or equivalent single-flight guard.
- Reentrancy is disallowed initially.
- Concurrent Tool/Ingress calls to the same instance are queued or rejected with bounded diagnostics.
- Timeout/trap/cancel marks the instance failed and either restarts according to policy or returns a bounded error.
- Crash/restart/backoff/status are visible in diagnostics, not hidden in Tool prose.

## Runtime compatibility

Implement all active paths in one change set, but preserve compatibility:

- New instance world:
  - `world = "yoi:plugin/instance@1.0.0"` or equivalent.
  - Host keeps component instance alive across calls.
  - Tool/Ingress/Service dispatch share that instance.
- Existing Component Tool world:
  - Continue supporting `world = "yoi:plugin/tool@1.0.0"`.
  - Wrap as a legacy `PluginInstance` adapter.
  - Adapter may still instantiate per call internally, but ToolRegistry sees an instance dispatch target.
- Existing raw core-Wasm Tool runtime:
  - Continue supporting current `kind = "wasm"` / `abi = "yoi-plugin-wasm-1"` as a legacy Tool adapter.
  - No Service/Ingress support for raw ABI unless explicitly added later.

This avoids breaking existing Tool Plugin packages while moving Pod/plugin architecture to the instance model immediately.

## Manifest / package model

Extend manifest/static inspection so a package can declare instance-backed surfaces:

- `surfaces = ["tool"]` remains valid.
- `surfaces = ["tool", "service", "ingress"]` becomes valid when runtime/interface supports it.
- `[[tools]]` remains the model-visible Tool schema declaration source.
- Add Service declarations with id/name/description/lifecycle/required host APIs/side effects/status metadata.
- Add Ingress declarations with id/name/event kinds/input schema/source expectations/side effects/action outputs.
- Static validation must reject Service/Ingress declarations for runtimes/interfaces that cannot implement them.
- `yoi plugin check/list/show` must report Tool/Service/Ingress eligibility, grants, rejected surfaces, runtime compatibility, diagnostics, and whether a package is legacy Tool-only or instance-capable.

Permission/grant model:

- Surface grants stay explicit: `surface=tool`, `surface=service`, `surface=ingress`.
- Tool grants remain per Tool name.
- Service grants are per Service id/name.
- Ingress grants are per Ingress id/name and event source/kind where applicable.
- Host APIs (`https`, `fs`, future secrets/network/event sources) remain separately grant-gated.
- Output actions from Ingress/Service must be separately constrained; Plugin instance access does not imply arbitrary Yoi mutation authority.
- SDK/PDK helper availability is never authority.

## Tool / Service / Ingress interaction

- Tool processing must be able to access the same Plugin instance used by Service/Ingress.
- Tool execution remains model/user initiated and returns through the ordinary Tool result/history path.
- Service/Ingress must not secretly call model Tools or mutate context/history directly.
- Service/Ingress may return host-mediated actions; host applies only approved actions to explicit durable/visible paths.
- Long-running work started from a Tool should return an accepted/status result and continue through instance/service diagnostics or explicit action paths rather than blocking the Tool indefinitely.

## Ingress and action paths

Ingress is the boundary for external events entering Yoi.

- Events are untrusted, bounded, typed inputs.
- Event delivery must not insert hidden context.
- Allowed outputs must be explicit action variants, initially limited to safe host-mediated paths such as diagnostics/status and, if implemented, Notify/SystemItem/Companion-visible messages through existing durable mechanisms.
- Arbitrary UI channels are prohibited.
- If an action path does not yet have a safe host API, the implementation should expose diagnostics/status rather than inventing an unsafe path.

## Implementation scope

This Ticket is no longer design-only. Implement the complete instance-oriented plugin runtime boundary in one integrated change:

1. Instance model and registry in Pod plugin feature code.
2. ToolRegistry dispatch through Plugin instance handles.
3. New Component instance world/resource files and Rust PDK support.
4. Legacy Tool component/raw wasm adapters behind the instance registry.
5. Manifest/static validation additions for Service/Ingress declarations.
6. `yoi plugin check/list/show` reporting updates.
7. Host-managed start/status/stop lifecycle and bounded diagnostics.
8. Ingress dispatch API and at least one test/in-process ingress delivery path.
9. Grant checks for Tool/Service/Ingress surfaces and host APIs.
10. Docs/templates updated so Tool-only vs instance-capable Plugin authoring is explicit.

Discord Bridge itself remains out of scope, but the implemented runtime must be sufficient for a Discord Bridge Plugin to be designed on top of it without another foundational runtime redesign.

## Non-goals

- Implementing Discord Bridge itself.
- Granting raw ambient WASI network/socket access to Plugin code.
- Public registry/install/update/signature tooling.
- Arbitrary Plugin UI channel.
- Hidden prompt/context injection.
- Allowing Service/Ingress to mutate model-visible Tool schema mid-run.

## 受け入れ条件

- Pod/plugin architecture uses a `PluginInstanceRegistry` or equivalent host-managed instance boundary.
- Existing Tool Plugin packages still work through the instance registry compatibility path.
- A new instance-capable Component Plugin can expose at least one Tool and have multiple Tool calls reach the same Plugin instance during a Pod run.
- A new instance-capable Component Plugin can expose at least one Ingress handler and receive a bounded in-process test event through the registry.
- Service lifecycle/status is represented and visible in inspection/diagnostics, even if the first implementation only supports host-managed start/status/stop and no real external socket source.
- Tool / Service / Ingress grants are validated independently; sharing a Plugin instance does not bypass per-surface authorization.
- Tool schemas remain run-stable and model-visible only through normal ToolRegistry construction.
- Plugin output/event effects use Tool results or explicit durable/visible host-mediated paths; no hidden context injection is introduced.
- `yoi plugin check/list/show` distinguishes legacy Tool-only packages from instance-capable packages and explains rejected Service/Ingress surfaces.
- Rust PDK/template/docs include instance-oriented authoring in addition to legacy/convenience Tool-only authoring.
- Focused tests cover:
  - manifest validation for Tool/Service/Ingress declarations;
  - legacy Tool component execution through the instance registry;
  - instance state persistence across two Tool calls;
  - Tool call dispatch to the same instance used by Ingress delivery;
  - grant denial for missing Service/Ingress grants;
  - timeout/trap/failure diagnostics for an instance call.
- Validation before completion includes `cargo fmt --check`, relevant `cargo test`, `cargo check`, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`.
