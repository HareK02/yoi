<!-- event: create author: tickets.sh at: 2026-06-03T12:23:17Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-03T16:38:46Z -->

## Plan

# Delegation intent: Plugin base Pod API design

## Intent

Design the public Pod-side API that will serve as the base for Plugin / Feature contributions. The result should make Plugin-provided or built-in extension modules easy to register cleanly without adding ad hoc Pod processing paths.

This is a design task, not an implementation task. The output should be a concise but concrete design document suitable for turning into implementation tickets or acceptance criteria for `plugin-feature-contribution-registry` and `hook-public-surface-hardening`.

## Background

The current direction is that feature state remains owned by the feature/extension module, while interaction with Pod happens through existing durable host surfaces:

- Tools
- Hooks
- notifications / events / durable history append paths

The concern is that adding WorkItem, MCP, memory, plugin, and other capabilities without a common registry will create many unrelated Pod-specific insertion points. The Plugin system should establish a common contribution and authority boundary, even for built-in features.

`hook-public-surface-hardening` is being implemented separately to make public Hook actions safe before plugin exposure.

## Design question

What should the clean public API look like for a feature/plugin module that wants to contribute capabilities to a Pod?

The design should answer:

- What API types should extension modules use to declare/register capabilities?
- What belongs in a pure descriptor vs a runtime install callback?
- How should Tools, Hooks, and notifications be represented in the same public surface?
- How should capability request / host grant / diagnostics be expressed?
- What state should the feature keep itself, and what state may Pod keep?
- What must be impossible through this API?
- Where should the API live initially, and what parts should be movable to a future `plugin`/`extension` crate?

## Required constraints

- Public API must not let features/plugins mutate prompt context or session history invisibly.
- Model-visible additions must go through durable host paths: tool result, committed history append, explicit notification/history append, or user-visible event path.
- Public Hook contribution must depend on the safe Hook surface after `hook-public-surface-hardening`.
- Tool contributions must use the normal ToolRegistry / PreToolCall permission / history result path.
- Feature registry must install into existing Pod/Worker surfaces; it must not create a parallel Pod runtime path.
- Capability grant is host-controlled. A feature may request capabilities but must not assume them.
- Built-in features and future external plugins should fit the same shape.
- Avoid designing package distribution, WASM execution, or MCP implementation details beyond the minimal runtime-kind placeholders needed for the API.
- Avoid broad refactors of Pod/Worker crate boundaries unless needed to explain a clean API boundary.

## Files / records to read

Tickets:

- `/home/hare/Projects/yoi/work-items/open/20260603-122317-plugin-feature-contribution-registry/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260603-122317-hook-public-surface-hardening/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260531-010005-plugin-extension-surface/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260601-031252-builtin-work-item-intake-routing/item.md`

Code:

- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/permission.rs`
- `crates/llm-worker/src/tool.rs`
- `crates/llm-worker/src/interceptor.rs`
- `crates/tools/src/lib.rs`
- `crates/pod/src/workflow/mod.rs`

## Expected output

Write a design document to:

`/home/hare/Projects/yoi/work-items/open/20260603-122317-plugin-feature-contribution-registry/artifacts/pod-api-design.md`

Use this structure:

1. Summary recommendation
2. Current relevant Pod/Worker surfaces
3. Proposed public API shape
   - types/modules
   - example registration snippet
   - Tool contribution
   - Hook contribution
   - notification/event contribution
   - capability request/grant/diagnostics
4. State ownership model
5. Safety invariants / forbidden operations
6. Placement and crate-boundary recommendation
7. Migration path from current built-in registrations
8. Impact on WorkItem / MCP / plugin distribution follow-ups
9. Open questions / risks

## Non-goals

- Do not edit source code.
- Do not implement tests.
- Do not create a worktree.
- Do not close or modify tickets except writing the requested design artifact.

## Completion report

Report:

- whether the artifact was written
- the recommended API placement
- the highest-risk API decision
- any blockers that require parent/user decision


---

<!-- event: plan author: hare at: 2026-06-03T16:44:05Z -->

## Plan

# Public Pod-side API for Feature / Plugin Contributions

## 1. Summary recommendation

Introduce a `pod::feature` public API as the single Pod-side registration layer for built-in features and future external plugins. A feature module should declare its identity, requested capabilities, and contributions, then install those contributions only through typed host registrars for existing Pod/Worker surfaces: `ToolRegistry`, the hardened safe `pod::hook` surface, and host-owned notification/event/history append paths.

The registry should not become a second runtime, a plugin dispatcher tool, or a generic `Pod` mutation escape hatch. Feature state remains inside the feature module; the Pod owns only install metadata, diagnostics, granted host handles, and normal durable session/runtime surfaces.

Recommended placement: create `crates/pod/src/feature.rs` (or `crates/pod/src/feature/mod.rs` once it grows) and export it as `pod::feature`. Keep `llm-worker::Interceptor` internal; expose only hardened `pod::hook` types and contribution registrars.

## 2. Current relevant Pod/Worker surfaces

The design should build on these existing surfaces rather than bypassing them:

- `crates/pod/src/hook.rs`
  - Current public-ish hook layer wraps `llm_worker::Interceptor` with `HookRegistry`, `HookRegistryBuilder`, `Hook`, and per-event hook traits.
  - It already provides Pod-specific hook events such as pre-request, post-assistant, pre-tool-call, post-tool-call, and turn-end.
  - It is not yet safe enough as a public plugin API because some hook actions can carry raw `llm_worker::Item` values (`PreRequestAction::ContinueWith`, `TurnEndAction::ContinueWithMessages`). The feature API must depend on the post-hardening surface, not these raw item mutation forms.

- `crates/pod/src/ipc/interceptor.rs`
  - `PodInterceptor` is the bridge between Worker callbacks and Pod behavior.
  - It runs hooks, drains pending attachments/notifications, records memory/tool usage, and turns model-visible additions into committed `SystemItem` session log entries before appending them to Worker history.
  - This is the right place for host-mediated durable append paths; it is not a plugin API itself.

- `crates/pod/src/controller.rs`
  - Controller startup currently registers built-in Pod tools through ad hoc code paths.
  - The feature registry should replace those ad hoc registrations incrementally by installing contributions into the same worker/tool/hook surfaces during Pod construction.

- `crates/pod/src/pod.rs`
  - `Pod` owns the durable session log, metadata, runtime event channel, notification helpers, pending system attachments, scope, and Worker lifecycle.
  - It exposes internal methods that can append history or send alerts/events. The public feature API should not expose `Pod` or `Worker` directly; it should expose narrow sinks that route through these existing methods.

- `crates/pod/src/permission.rs`
  - Manifest tool permissions are enforced as a `PreToolCallHook`.
  - Feature tools must remain subject to the same PreToolCall permission path. Feature capability grants do not replace per-call tool permission.

- `crates/llm-worker/src/tool.rs` and `crates/llm-worker/src/tool_server.rs`
  - `ToolDefinition`, `Tool`, `ToolMeta`, `ToolResult`, `ToolOutput`, and `ToolServerHandle` define the normal tool execution path.
  - Tools registered here get normal schema exposure, execution, bounded output handling, and history result recording.
  - The public feature API should register `ToolDefinition`s into this registry rather than introducing a separate plugin dispatch layer.

- `crates/llm-worker/src/interceptor.rs`
  - The lower-level interceptor is powerful and Worker-oriented. It should remain internal because it can influence model request construction too directly.
  - Public features should use `pod::hook` only after that API has been narrowed to durable, auditable actions.

- `crates/tools/src/lib.rs`
  - Existing built-in tools already use shared tool abstractions and scoped filesystem/runtime handles.
  - Those tool constructors can become built-in feature contributions without changing model-visible tool names.

- `crates/pod/src/workflow/mod.rs`
  - Workflow invocation currently resolves user input segments into system items through the Pod's durable attachment path.
  - This is a useful pattern for feature-owned model-visible additions: resolve through a host-owned append path and commit what the model sees. It should not become a general plugin context injection mechanism.

## 3. Proposed public API shape

### Types/modules

Add a new module under `pod`:

```rust
pub mod feature {
    pub mod capability;
    pub mod diagnostic;
    pub mod event;
    pub mod hook;
    pub mod registry;
    pub mod tool;

    pub use capability::{CapabilityGrantSet, CapabilityRequest, HostCapability};
    pub use diagnostic::{FeatureDiagnostic, FeatureInstallReport};
    pub use registry::{FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureModule, FeatureRegistryBuilder, FeatureRuntimeKind};
    pub use tool::ToolContribution;
}
```

Core trait and registry shape:

```rust
pub trait FeatureModule: Send + Sync + 'static {
    fn descriptor(&self) -> FeatureDescriptor;

    fn install(&self, ctx: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError>;
}

pub struct FeatureDescriptor {
    pub id: FeatureId,                  // source-qualified identity, e.g. builtin:task
    pub display_name: String,
    pub version: Option<String>,
    pub runtime: FeatureRuntimeKind,    // Builtin, ExternalProcess, McpBridge, WasmPlaceholder, DeclarativePlaceholder
    pub requested_capabilities: Vec<CapabilityRequest>,
    pub declared_tools: Vec<ToolDeclaration>,
    pub declared_hooks: Vec<HookDeclaration>,
    pub declared_event_channels: Vec<EventChannelDeclaration>,
}

pub enum FeatureRuntimeKind {
    Builtin,
    ExternalProcess,
    McpBridge,
    WasmPlaceholder,
    DeclarativePlaceholder,
}

pub struct FeatureInstallContext<'a> {
    // No Pod or Worker reference.
    pub feature_id: &'a FeatureId,
    pub grants: &'a CapabilityGrantSet,
    pub tools: ToolRegistrar<'a>,
    pub hooks: PublicHookRegistrar<'a>,
    pub notify: FeatureNotifySink<'a>,
    pub events: FeatureEventSink<'a>,
    pub diagnostics: FeatureDiagnosticSink<'a>,
    pub services: FeatureServiceProvider<'a>,
}
```

Important details:

- `FeatureDescriptor` is declarative and serializable. It is safe to show in diagnostics, profile previews, and `ListFeatures`-style future tooling.
- `FeatureModule::install` is runtime code that wires stateful tool/hook implementations into host registrars.
- `FeatureInstallContext` must not expose `Pod`, `Worker`, raw `ToolServerHandle`, raw `Interceptor`, raw `NotifyBuffer`, raw `LogWriter`, raw `event_tx`, or direct history mutation.
- `FeatureServiceProvider` returns only host services backed by granted capabilities, for example scoped filesystem access, WorkItem store access, memory access, Pod orchestration handles, web provider handles, or secret references. It should return `Denied`/`Unavailable` diagnostics instead of exposing partial internals.

### Example registration snippet

This is illustrative shape, not proposed final exact Rust syntax:

```rust
use pod::feature::{
    CapabilityRequest, FeatureDescriptor, FeatureId, FeatureInstallContext,
    FeatureModule, FeatureRuntimeKind, HostCapability, ToolContribution,
};

pub struct WorkItemFeature {
    state: std::sync::Arc<WorkItemFeatureState>,
}

impl FeatureModule for WorkItemFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builder(FeatureId::builtin("work-item"))
            .display_name("WorkItem intake and routing")
            .runtime(FeatureRuntimeKind::Builtin)
            .request(CapabilityRequest::required(
                HostCapability::WorkItemStore { read: true, write: true },
                "create and update WorkItem records through host-owned ticket storage",
            ))
            .request(CapabilityRequest::optional(
                HostCapability::EmitUserEvent,
                "surface routing diagnostics to the TUI/actionbar",
            ))
            .tool("WorkItemCreate")
            .tool("WorkItemComment")
            .hook("work_item_intake_pre_tool_audit", pod::hook::HookPoint::PreToolCall)
            .event_channel("work-item")
            .build()
    }

    fn install(&self, ctx: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let store = ctx.services.work_item_store()?;

        ctx.tools.register(ToolContribution::new(
            "WorkItemCreate",
            work_item_create_tool(store.clone(), self.state.clone()),
        ))?;

        ctx.hooks.pre_tool_call(
            "work_item_intake_pre_tool_audit",
            WorkItemAuditHook::new(self.state.clone()),
        )?;

        ctx.events.declare_channel("work-item")?;
        Ok(())
    }
}
```

The feature keeps `WorkItemFeatureState`. The Pod keeps only registration records, diagnostics, and the normal host services it already owns.

### Tool contribution

A tool contribution should be a thin wrapper around `llm_worker::ToolDefinition` plus feature metadata:

```rust
pub struct ToolContribution {
    pub feature_id: FeatureId,
    pub name: ToolName,
    pub definition: llm_worker::ToolDefinition,
    pub required_capabilities: Vec<HostCapability>,
}
```

Rules:

- Register into the existing `ToolRegistry` / `ToolServerHandle`; do not add a plugin-dispatcher tool that multiplexes plugin calls outside normal tool history.
- Preserve normal `PreToolCall` permission evaluation, tool-call history, result history, output truncation/bounding, and diagnostic behavior.
- Host-controlled feature enablement decides whether a contributed tool is installed. Manifest/profile tool permission still decides whether a model may call it at runtime.
- Duplicate tool names should be rejected during feature registry preflight with a diagnostic, not discovered later through a panic or undefined ordering.
- Public feature identity should be source-qualified (`builtin:memory`, `project:foo`, `plugin:<digest>:bar`), while model-visible tool names should remain explicit stable names. Do not auto-prefix model tool names unless the project deliberately chooses a future namespacing policy.
- Tool schemas/descriptions must be part of the normal `ToolDefinition` path so model-visible surfaces remain inspectable and bounded.
- If a required host service is not granted or configured, the tool should not be registered; the install report should explain the skipped contribution.

### Hook contribution

Hook contribution must depend on the safe hook surface produced by `hook-public-surface-hardening`.

Recommended public hook principles:

- Public hooks register through `PublicHookRegistrar`, which wraps `HookRegistryBuilder` but exposes only hardened hook traits/actions.
- Public hooks receive snapshots/views, not mutable Pod/Worker handles.
- Public hook return values should be decisions such as continue, deny/rewrite a tool decision through a host-defined synthetic result path, emit diagnostics, or request a durable notification/history append through a host sink. They should not return raw `llm_worker::Item` vectors.
- Public hooks must not be able to mutate request context, session history, or Worker state invisibly.
- Permission enforcement hooks remain host/internal and run before feature hooks for `PreToolCall` so a feature cannot approve a denied tool call.
- Hook ordering should be explicit and stable: internal safety hooks first, public feature hooks in registry order or declared priority bands, internal usage/accounting hooks where needed. Priority should be coarse, not arbitrary integer ordering that lets plugins fight for precedence.

Possible hardened hook action shape:

```rust
pub enum PublicPreToolCallDecision {
    Continue,
    DenyWithSyntheticError { message: String },
    EmitDiagnostic { diagnostic: FeatureDiagnostic },
}

pub trait PublicPreToolCallHook: Send + Sync {
    fn on_pre_tool_call(&self, event: PublicPreToolCallEvent<'_>) -> PublicPreToolCallDecision;
}
```

If a hook needs to add model-visible text, it should use `FeatureNotifySink::notify_model(...)` or another host-owned durable append API, not return an `Item`.

### Notification/event contribution

Expose two distinct sinks:

```rust
pub struct FeatureNotifySink<'a> { /* host-owned */ }
pub struct FeatureEventSink<'a> { /* host-owned */ }
```

Recommended behavior:

- `FeatureNotifySink::notify_model(...)` creates a model-visible notification through the existing durable notification/system-item path. The host commits the corresponding `SystemItem` before it is appended to Worker history.
- `FeatureNotifySink::notify_user(...)` or `FeatureEventSink::emit(...)` creates user-visible diagnostics/progress/action events through the existing alert/event path. These are not model-visible unless explicitly routed through `notify_model`.
- Event payloads should be typed, bounded, and feature-identified. Avoid arbitrary JSON blobs as the first public API; allow an opaque bounded metadata field only if diagnostics require it.
- Notifications and events should require explicit capabilities such as `EmitModelNotification` and `EmitUserEvent`.
- Background feature tasks must use these sinks; they must not hold raw log writers or append directly to history.

Useful initial event shape:

```rust
pub struct FeatureEvent {
    pub feature_id: FeatureId,
    pub level: FeatureEventLevel,       // Info, Warn, Error
    pub channel: String,                // e.g. "work-item"
    pub summary: String,
    pub detail: Option<String>,
    pub model_visible: bool,            // false unless host routes through notify_model
}
```

`model_visible` should be host-controlled in practice: a feature may request model visibility, but the sink decides whether that capability is granted and records the durable append if it is.

### Capability request/grant/diagnostics

Capabilities are requested by descriptors and granted by the host. A feature may request a capability, but it must not assume the capability exists.

Initial capability categories:

```rust
pub enum HostCapability {
    ContributeTool { name: ToolName },
    ContributeHook { point: pod::hook::HookPoint },
    EmitUserEvent,
    EmitModelNotification,
    ScopedFs { read: bool, write: bool, execute: bool },
    WorkItemStore { read: bool, write: bool },
    MemoryStore { read: bool, write: bool },
    PodManagement { spawn: bool, message: bool, restore: bool },
    Network { purpose: NetworkPurpose },
    SecretRef { id: String },
}
```

Important separation:

- Capability grants decide whether a feature may install and receive host services.
- Tool permissions decide whether an installed tool call may execute for a specific Pod/run.
- Scope permissions decide which filesystem paths or delegated Pod capabilities a host service may touch.

Diagnostics should be first-class:

```rust
pub struct FeatureInstallReport {
    pub feature_id: FeatureId,
    pub enabled: bool,
    pub granted: Vec<HostCapability>,
    pub denied: Vec<CapabilityDenial>,
    pub installed_tools: Vec<ToolName>,
    pub installed_hooks: Vec<String>,
    pub skipped_contributions: Vec<SkippedContribution>,
    pub diagnostics: Vec<FeatureDiagnostic>,
}
```

Diagnostics must avoid secrets and must be safe for session logs, TUI display, and future `ListFeatures`/profile validation output.

## 4. State ownership model

Feature state belongs to the feature module.

- A feature may own `Arc<State>` and clone it into contributed tools, hooks, and background tasks.
- The Pod registry stores descriptors, install reports, enabled/disabled status, and host-owned handles. It does not store feature business state.
- Durable feature data must live in a feature-owned or host-granted store with an explicit API: WorkItem files through a WorkItem service, memory records through memory APIs, plugin config/state through a future plugin-state service, etc.
- Session history is not feature storage. It is an audit/replay record of model-visible interactions and host-visible events.
- A feature that needs restoration after process restart should reconstruct itself from its own durable store/config plus normal Pod metadata, not from private data hidden in Worker context.
- Background tasks are allowed only if they communicate through granted sinks/services and have a defined shutdown/lifecycle policy owned by the host.

This model lets built-ins and plugins share the same contribution shape while keeping Pod runtime ownership clear.

## 5. Safety invariants / forbidden operations

Public features/plugins must not be able to perform these operations:

- Mutate prompt context directly.
- Append, remove, reorder, or rewrite Worker history directly.
- Insert model-visible text that is not committed through a durable host path.
- Return raw `llm_worker::Item` values from public hooks.
- Access raw `Worker`, raw `Pod`, raw `ToolServerHandle`, raw `llm_worker::Interceptor`, raw `NotifyBuffer`, raw session log writer, or raw event sender.
- Register tools outside `ToolRegistry` or bypass normal tool-result history recording.
- Bypass `PreToolCall` permission policy.
- Grant themselves capabilities or infer grants from successful construction.
- Mutate manifest/profile/scope state directly.
- Perform filesystem/process/network/secret access outside granted host services.
- Emit unbounded tool outputs, event payloads, diagnostics, or notification bodies.
- Put secrets into diagnostics, session logs, model context, TUI output, or feature install reports.
- Depend on MCP/WASM/package-distribution mechanics in the base Pod API.

Positive invariant: if the model can see a feature-produced fact, a future replay/resume must have a durable explanation for why that fact was present.

## 6. Placement and crate-boundary recommendation

Recommended placement:

- `crates/pod/src/feature.rs` or `crates/pod/src/feature/mod.rs`
  - public feature traits/types
  - feature registry builder
  - install reports/diagnostics
  - capability request/grant model
  - typed registrars/sinks

- `crates/pod/src/hook.rs`
  - remains the public hook module after hardening
  - should expose safe Pod-level hook traits/actions only
  - should not re-export `llm_worker::Interceptor` power

- `crates/llm-worker`
  - remains owner of generic LLM tools/interceptors/history machinery
  - should not depend on `pod::feature`

- `crates/tools`
  - remains a source of reusable tool implementations
  - built-in feature modules in `pod` can wrap these constructors into `ToolContribution`s

- Future external plugin crates/processes
  - should adapt into `FeatureDescriptor` + `FeatureModule` or a host-side adapter that produces equivalent contributions
  - should not be called directly by the Pod except through the registry/registrars

Install location in Pod startup:

1. Resolve manifest/profile and host capability policy.
2. Construct `Pod` and internal safety surfaces.
3. Install host/internal hooks such as manifest permission enforcement.
4. Build and install enabled feature modules through `FeatureRegistryBuilder`.
5. Flush/register tools through the existing Worker tool registry.
6. Freeze/install the Pod interceptor and start normal run/attach behavior.

The exact sequencing can be adjusted to match current construction, but the invariant should hold: public feature hooks cannot precede host safety hooks, and feature tools must exist before the model receives the final tool schema for a run.

## 7. Migration path from current built-in registrations

Recommended migration is incremental and behavior-preserving:

1. Land hook public-surface hardening first.
   - Remove/replace public raw `Item`-carrying hook actions.
   - Define which hook decisions are safe for external contributors.

2. Add `pod::feature` with no behavior change.
   - Implement descriptors, capability grants, install reports, and registrars.
   - Initially register no external plugins.

3. Wrap current built-in tool registration as built-in feature modules.
   - Start with a small built-in feature whose state/services are already cleanly bounded.
   - Preserve existing tool names, schemas, and permission behavior.
   - Convert duplicate-name failures into registry diagnostics before flushing tools.

4. Move larger built-in groups behind feature modules.
   - Filesystem/process tools from `crates/tools`.
   - Memory tools.
   - Pod orchestration tools.
   - Task/WorkItem tools once their stores and hooks have explicit capabilities.
   - Web tools as configured provider-backed features.

5. Move built-in hook contributions only after safe hook semantics are stable.
   - Keep manifest permission enforcement as an internal host hook, not a feature hook.
   - Keep accounting/usage hooks internal unless they become genuine feature behavior.

6. Treat workflow/user-input expansion separately.
   - Workflow invocation already uses a durable system-item attachment pattern.
   - Do not expose arbitrary workflow-like context injection to plugins until there is a safe typed command/input-contribution API with durable append semantics.

7. Add profile/manifest enablement after built-ins work through the same registry.
   - Built-ins and external plugins should share descriptor/capability/install-report mechanics.
   - Host policy may grant built-ins by default, but built-ins should still declare what they use.

## 8. Impact on WorkItem / MCP / plugin distribution follow-ups

WorkItem / intake routing:

- WorkItem routing can become a built-in feature that contributes WorkItem tools, optional routing hooks, and user-visible action events.
- It should request `WorkItemStore` and event/notification capabilities instead of reaching into ticket files ad hoc.
- Model-visible routing hints or intake results must be committed through notification/history append paths.
- This registry gives the WorkItem feature a clean way to install without making WorkItem a special Pod runtime mode.

MCP:

- MCP should be an adapter/runtime kind that produces normal `ToolContribution`s and possibly safe event diagnostics.
- MCP tool calls must still pass through `ToolRegistry`, PreToolCall permission, output bounding, and history result recording.
- MCP resources/prompts should not become invisible prompt injection. If exposed later, they should be explicit tools, user-invoked attachments, or durable notification/history appends.
- MCP transport/session details are out of scope for the base API beyond the `FeatureRuntimeKind::McpBridge` placeholder.

Plugin distribution:

- Archive validation, cache extraction, signing/trust, WASM execution, external process supervision, and package update policy should remain separate follow-up designs.
- Distribution mechanisms should eventually produce the same descriptor/capability/contribution objects as built-ins.
- Capability grants are the host trust boundary; package installation alone must not grant runtime authority.

## 9. Open questions / risks

1. Tool naming policy is the highest-risk API decision.
   - Recommendation: feature identities are source-qualified, model-visible tool names stay explicit and stable, and collisions are rejected by the host.
   - Risk: external plugins may need namespacing later. Auto-prefixing now would avoid collisions but would also change model-facing ergonomics and diverge from current built-in tool names.

2. The exact safe hook action set must be settled by `hook-public-surface-hardening`.
   - Especially important: whether public pre-tool hooks may synthesize denials/results, and how durable append requests are represented.

3. Notification/event durability needs precise semantics.
   - User-visible events may be live-only, while model-visible notifications must be durable. The public API should make this distinction impossible to miss.

4. Capability granularity can easily become either too coarse or too noisy.
   - Start with coarse host-service capabilities plus normal tool permissions, then split only when real features need finer grants.

5. Runtime enable/disable is not designed here.
   - Initial registry should be install-at-startup. Hot reload or dynamic plugin enablement needs separate lifecycle, cleanup, and schema-refresh design.

6. Persistent plugin state needs a future host service.
   - The base API says state is feature-owned, but external plugins will still need a sanctioned durable state directory/store with migration/versioning rules.

7. Background tasks need lifecycle policy.
   - If external plugins can spawn tasks, the host must define shutdown, cancellation, panic handling, diagnostic routing, and whether task output may become model-visible.

8. Existing workflow/input expansion is close to the forbidden boundary.
   - It is safe only because it commits system items before model visibility. Any future plugin command/input contribution must preserve that durable replay property.


---

<!-- event: decision author: hare at: 2026-06-04T20:23:15Z -->

## Decision

# Decision: remove generic event channels and standardize BackgroundTask

The Plugin/Feature base Pod API should not expose a plugin-defined event-channel mechanism for arbitrary structured client/UI payloads.

Revised boundary:

- Model-visible notifications use the existing durable `Method::Notify` / `SystemItem::Notification` / `Event::SystemItem` path. If the model can see it, it must be committed to history and visible to users on replay/inspection.
- `Event::Alert`-like output is a short transient human-facing alert only. It is not model-visible, not session history, and not a structured UI extension channel.
- Diagnostics/status are host-defined operational records for install/runtime/capability/task reporting, not arbitrary plugin UI messages.
- Dialogs/confirmations/custom UI are deferred. If needed later, they should be a separate host-defined interaction protocol, not a generic plugin event channel.
- `BackgroundTaskContribution` is a first-class contribution kind. The host starts, tracks, cancels, and reports background tasks; feature modules must not spawn untracked async loops. Task output is limited to granted sinks/services: model notification, alert, diagnostics, and host-granted services.

This keeps the Plugin API centered on Tools, safe Hooks, host-managed BackgroundTasks, durable model-visible notifications, and bounded host-defined operational reporting.


---

<!-- event: decision author: hare at: 2026-06-04T20:48:19Z -->

## Decision

# Decision: add host-mediated Feature services

Add Service provider/consumer support to the Plugin/Feature base Pod API.

This is not a decision to extract currently implemented core features such as Memory or Pod management immediately. Existing implementations may remain core-backed. The new service form exists so future built-in features and plugins can expose stable APIs to other features without direct concrete dependencies or ad hoc Pod internals access.

Revised contribution/dependency model:

- Contributions:
  - ToolContribution
  - HookContribution
  - BackgroundTaskContribution
  - ServiceProvider / ServiceDeclaration
- Dependencies:
  - ServiceRequirement, resolved by the host registry before feature installation

Rules:

- A feature/plugin may provide a public service through a host-owned service registry.
- Another feature/plugin may acquire that service only through the host, after dependency resolution and capability grant checks.
- Consumers do not import provider concrete types, private state, raw process handles, raw WASM/MCP handles, or plugin-specific modules.
- Required missing services skip the consuming feature with diagnostics; optional missing services allow degraded installation when supported.
- Service cycles are rejected initially.
- In-process built-ins may use Rust trait-object handles internally, but the public design must leave room for external plugin service proxies.
- Service handles must be capability-bound so acquiring a broad service does not become an authority escalation path.

Examples:

- `builtin:memory` may provide `yoi.memory.v1`; other features can optionally consume read-only memory lookup without depending on Memory internals.
- `builtin:pod-orchestration` may provide `yoi.pod-management.v1` as a controlled façade while the actual Pod lifecycle/scope authority remains host-owned.
- Future issue-tracker plugins may provide `project.issue-tracker.v1` for WorkItem integration.


---

<!-- event: plan author: hare at: 2026-06-04T21:08:33Z -->

## Plan

# Delegation intent: plugin feature contribution registry implementation

## Intent

Implement the first behavior-preserving slice of `plugin-feature-contribution-registry`: add a Pod-side Feature/Plugin contribution boundary that can represent built-in and future external capabilities without creating ad hoc Pod insertion paths.

This implementation should establish the API skeleton and prove the installation path with at least one small built-in capability group. It should not attempt to implement external plugin loading, package distribution, WASM, MCP, WorkItem tools, or broad migration of all built-in tools.

## Scope for this implementation

Implement a focused Phase 1/2 slice:

1. Add `pod::feature` module structure and public types for:
   - `FeatureId`
   - `FeatureRuntimeKind`
   - `FeatureDescriptor`
   - `FeatureModule`
   - `FeatureInstallContext`
   - `FeatureInstallReport`
   - diagnostics / skipped contributions
   - capability request/grant data
   - tool contribution wrapper
   - safe hook contribution registrar shape, using the already-hardened `pod::hook` surface
   - background task declaration / contribution skeleton
   - service declaration / service requirement / service registry skeleton
   - notification/alert/diagnostic sink skeletons where needed by the install context
2. Add a registry/builder/install path that can install enabled feature modules into existing host surfaces.
   - Tool contributions must end up in the normal Worker/ToolRegistry path.
   - Hook contributions must go through `HookRegistryBuilder` / safe `pod::hook` APIs.
   - BackgroundTask and Service APIs may be skeleton/diagnostic-only if full runtime lifecycle would be too large, but their descriptors and install reports must be represented.
3. Migrate one small, low-risk built-in tool/capability group through the registry to prove behavior without changing model-visible behavior.
   - Preserve tool name/schema/permission behavior exactly.
   - Prefer a group with minimal state and no complicated runtime lifecycle.
   - If no suitable group is obvious after inspection, implement a no-op built-in diagnostic feature and explicitly explain why; but prefer a real existing built-in registration if feasible.
4. Add focused tests for:
   - descriptor/capability/install report behavior
   - duplicate tool-name diagnostics/rejection
   - service requirement resolution basics: required missing -> skip/error diagnostic, optional missing -> degraded diagnostic if represented
   - installed built-in tool remains registered through the normal path
   - no direct public exposure of raw `Pod`, `Worker`, `ToolServerHandle`, `Interceptor`, raw history writer, raw event sender, or raw NotifyBuffer through `FeatureInstallContext`

## Required design constraints

Follow the current design records:

- `work-items/open/20260603-122317-plugin-feature-contribution-registry/item.md`
- `work-items/open/20260603-122317-plugin-feature-contribution-registry/artifacts/pod-api-design.md`
- `work-items/open/20260603-122317-plugin-feature-contribution-registry/artifacts/notification-background-task-revision.md`
- `work-items/open/20260603-122317-plugin-feature-contribution-registry/artifacts/service-registry-revision.md`

Core requirements:

- Do not create a generic plugin event channel.
- Do not implement custom UI/dialog payloads.
- Model-visible notifications must use the existing durable Notify/SystemItem/Event::SystemItem concept; do not add hidden context injection.
- `Event::Alert`-like output is only transient human-facing text.
- BackgroundTask is a first-class contribution concept, but host-managed lifecycle may be staged if needed.
- Services are host-mediated provider/consumer APIs; this is not a mandate to extract existing Memory or Pod management out of core.
- Feature-to-feature dependency must go through service declarations/requirements and host resolution, not concrete module/private state dependencies.
- Public feature API must not expose raw `llm_worker::Item` injection, raw internal interceptor actions, or arbitrary history/context mutation.
- Public hooks must use the hardened `pod::hook` safe action surface already merged by `hook-public-surface-hardening`.
- Feature capability grants do not replace manifest/tool permission checks.
- Existing behavior must remain unchanged except for internal registration plumbing and diagnostics.

## Non-goals

- External plugin discovery/loading.
- Plugin package format, archives, signing, extraction cache, or distribution.
- WASM runtime.
- MCP implementation.
- WorkItem tools/intake/orchestrator implementation.
- Moving Memory or Pod management implementation out of core.
- Hot reload / dynamic enable-disable.
- Generic UI/event channel or dialog protocol.
- Broad migration of all built-in tools in one pass.

## Suggested files to inspect

- `crates/pod/src/lib.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/hook.rs`
- `crates/pod/src/permission.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/llm-worker/src/tool.rs`
- `crates/llm-worker/src/tool_server.rs`
- `crates/tools/src/lib.rs`
- Existing built-in tool registration sites under `crates/pod/src/**`

## Escalate if

- Implementing even one real built-in feature migration requires broad rewiring of Worker/Pod construction.
- The service registry cannot be represented without committing to external-plugin ABI/proxy details.
- BackgroundTask lifecycle requires major runtime architecture decisions beyond a skeleton/descriptor/install-report path.
- A required design choice would change model-visible tool names, tool schemas, permission behavior, or history semantics.
- You find that the current `pod::hook` hardening is insufficient for a safe feature registrar.

## Validation

Run at least:

- focused tests added/updated for `pod::feature`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible. If not, report why.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- implemented feature API surface
- which built-in capability group was migrated/proven through the registry
- behavior-preservation notes
- service/background-task support level: full runtime vs descriptor/skeleton
- tests added/updated
- validation results
- unresolved risks / follow-up recommendations
- whether ready for external review


---

<!-- event: review author: hare at: 2026-06-04T21:32:42Z status: request_changes -->

## Review: request changes

# External sibling review: plugin-feature-contribution-registry

## 1. Result: request changes

Request changes. The implementation is a good focused first slice structurally, and the migrated Task tool group appears low-risk, but the public `pod::feature` boundary does not yet enforce its advertised authority/capability invariants consistently enough to merge as the plugin/feature registration boundary.

## 2. Summary of implementation

The coder commit `a8ae6ca feat: add pod feature registry slice` adds `crates/pod/src/feature.rs`, exposes it as `pod::feature`, and wires a `FeatureRegistryBuilder` into Pod tool registration. The existing generic builtin tool registration was split into:

- `tools::core_builtin_tools(...)` for filesystem/bash/web tools; and
- `tools::builtin_tools(...)` retaining the old all-tools helper for non-registry callers.

`TaskCreate`, `TaskUpdate`, `TaskGet`, and `TaskList` are migrated through a built-in `task_feature(...)` module. The new feature module includes descriptor/capability/report types, safe hook registrar shape, descriptor/report-only background tasks, descriptor/report-only services, and skeleton notification/alert/diagnostic sinks. Focused unit tests were added in `pod::feature` for install reporting, duplicate feature-tool names, service requirement basics, and Task tool registration through the Worker path.

## 3. Requirement-by-requirement assessment

- Feature identity/runtime metadata: mostly satisfied for the first slice. `FeatureId` and `FeatureRuntimeKind` exist, though runtime/source kinds are still coarse and can be extended later.
- Contribution descriptors for Tools/Hooks/BackgroundTasks/Services/notification-alert-diagnostic: partially satisfied. The types exist, but capability enforcement is inconsistent outside tool/hook registration.
- Capability request/grant data structures: partially satisfied. The data structures exist, but the install path currently grants all requested capabilities and several registrars/descriptors bypass the grant set entirely.
- Registry/builder/install path into existing host surfaces: mostly satisfied for tools and hooks. Tools are queued into the normal Worker registration path, and hooks go through `HookRegistryBuilder` with the hardened `pod::hook` action surface.
- Behavior preservation / migrated built-in proof: likely satisfied for the selected Task tools. Tool names are preserved in the focused test, and the migration is narrow.
- No raw Pod/Worker/ToolServerHandle/Interceptor/history/event/NotifyBuffer exposure through `FeatureInstallContext`: satisfied by code inspection for the public context surface. `Worker` is only used by a crate-private installer and tests.
- No generic plugin event channel or custom UI/dialog payload: satisfied. Notification/alert/diagnostic surfaces are skeleton/report-only and do not add a UI/event channel.
- Notification/background/service scope: mostly aligned as skeleton-level, but the service/capability boundaries need tightening before merge.
- Tests: useful but not sufficient for the safety boundary. Missing coverage for mismatched tool wrapper name vs model-visible `ToolMeta.name`, non-tool/hook capability denial, and code-level no-raw-handle exposure.

## 4. Blockers

### Blocker 1 — Tool capability/duplicate checks are keyed to a wrapper name that can diverge from the actual model-visible tool name

`ToolContribution::new(name, definition)` stores a caller-supplied `name` separately from the `ToolDefinition`'s eventual `ToolMeta.name` (`crates/pod/src/feature.rs:194-221`). `ToolContributionRegistrar::register` checks grants, duplicate names, and install reports only against that wrapper name, then pushes the uninspected `ToolDefinition` to the Worker (`crates/pod/src/feature.rs:666-698`).

That means a feature can be granted and reported for `SafeName` while the factory actually registers model-visible `OtherName` (or a duplicate of an existing/core tool) when the Worker flushes pending tools. This breaks the registry boundary's ability to preserve model-visible tool names/schemas and to diagnose/reject duplicate or ungranted tool contributions. It also leaves duplicate detection to the lower Worker flush panic for cases the registry should reject cleanly.

Before merge, the registry should make the tool name used for capability checks and reports the same authority as the model-visible `ToolMeta.name`, or otherwise validate the invariant before queuing the tool. The fix should include a focused regression test for a mismatched contribution name/factory name.

### Blocker 2 — Capability grants are not enforced for several advertised contribution surfaces

The implementation enforces grants for tools and hooks, but not for the rest of the public install context:

- `BackgroundTaskRegistrar::declare` records a background task without checking `HostCapability::DeclareBackgroundTask` (`crates/pod/src/feature.rs:777-785`).
- `FeatureServiceRegistrar::provide` registers a service without checking `HostCapability::ProvideService` (`crates/pod/src/feature.rs:788-801`).
- Descriptor-declared background tasks and provided services are copied/registered before `module.install(...)` without capability checks (`crates/pod/src/feature.rs:1012-1029`).
- `FeatureNotificationSink::notify_model`, `FeatureAlertSink::alert`, and `FeatureDiagnosticSink` are exposed without checking `EmitNotification`, `EmitAlert`, or `EmitDiagnostic` grants (`crates/pod/src/feature.rs:595-654`, `856-870`). They are skeleton/report-only today, but they still advertise host capabilities and should be denied/skipped consistently when not granted.

This makes `CapabilityGrantSet` unreliable as a host-mediated boundary: a feature can publish service/provider metadata or background task declarations without requesting/receiving the corresponding capability. Before merge, all advertised contribution/sink surfaces should either enforce the relevant grant or be explicitly non-capability-gated with the capability variants removed/deferred. Tests should cover denial/skipping for at least service and background-task contributions.

## 5. Non-blockers / follow-ups

- Service resolution is order-sensitive: required services only resolve against providers already registered by earlier modules (`missing_required_service` checks the current registry only). For this first descriptor skeleton it may be acceptable, but the design record describes host preflight/dependency resolution and initial cycle rejection. A follow-up should either topologically resolve against all descriptors or document that installation order is the dependency order for this slice.
- Existing/core Worker pending tool names are not incorporated into feature duplicate diagnostics. A future feature that collides with a core tool name would likely be caught only by Worker flush panic. This is related to Blocker 1 but also matters independently once more built-ins move behind the registry.
- The install report is currently assigned to `_feature_install_report` and discarded in `controller.rs`. That is acceptable for a tiny built-in migration, but discovery/enablement diagnostics should eventually be surfaced through a host-defined diagnostic path.
- Tests would be stronger with explicit compile/runtime checks that `FeatureInstallContext` does not expose raw `Pod`, `Worker`, `ToolServerHandle`, `Interceptor`, raw history/event/notify handles, or arbitrary `llm_worker::Item` injection.

## 6. Validation assessed or rerun

Reran:

- `git diff --check develop...HEAD` — passed.
- `git show --stat --oneline --decorate --no-renames a8ae6ca` — confirmed the reviewed commit and changed-file set.

Assessed by inspection:

- `git diff develop...HEAD`
- `crates/pod/src/feature.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/pod.rs`
- `crates/tools/src/lib.rs`
- `crates/pod/src/hook.rs`
- ticket item and supplied design/revision artifacts

I did not rerun `cargo test`, `cargo check`, or `nix build` in this external reviewer pass because the requested scope was source-read plus one artifact write, and the blockers above are API-boundary issues visible by inspection.

## 7. Residual risk

After the blockers are fixed, the main residual risk is that this first registry slice remains descriptor-heavy while actual host policy resolution is still minimal. That is acceptable for Phase 1/2 if the public API cannot misrepresent model-visible tools and if every exposed contribution/sink consistently goes through capability checks or is explicitly deferred.

---

<!-- event: review author: hare at: 2026-06-04T21:46:04Z status: request_changes -->

## Review: request changes

# External sibling rereview: plugin-feature-contribution-registry

## 1. Result: request changes

Request changes. The second blocker from the original review is addressed, but the first blocker is only partially fixed: the registry now compares the wrapper name against one materialization of the `ToolDefinition`, then queues the original factory for a second Worker materialization, so the checked name can still diverge from the actual model-visible registered tool name.

## 2. Summary of rereview

Reviewed the existing worktree/branch:

- Worktree: `/home/hare/Projects/yoi/.worktree/plugin-feature-contribution-registry`
- Branch: `work/plugin-feature-contribution-registry`
- Commits reviewed:
  - `a8ae6ca feat: add pod feature registry slice`
  - `4070176 fix: harden feature contribution gates`

The fix commit changes only `crates/pod/src/feature.rs`. It adds capability checks for background task, service, notification, alert, and diagnostic surfaces; adds service requirement capability checks; adds tool-name mismatch diagnostics; and expands focused tests.

## 3. Prior blocker assessment

### Prior blocker 1: Tool wrapper name vs actual model-visible tool name

Status: **not fully resolved**.

The new code in `ToolContributionRegistrar::register` does verify the wrapper name against a `ToolDefinition` materialization:

```rust
let model_visible_name = (contribution.definition)().0.name;
if contribution.name != model_visible_name { ... }
```

It then performs capability checks, duplicate checks, skipped/report entries, and `installed_tools` against that `model_visible_name`, which fixes the simple stable-factory mismatch case and is covered by the new test `mismatched_tool_contribution_name_is_rejected_before_queueing`.

However, after that validation it queues the original `ToolDefinition` factory unchanged:

```rust
self.pending_tools.push(contribution.definition);
```

`ToolDefinition` is an arbitrary `Arc<dyn Fn() -> (ToolMeta, Arc<dyn Tool>) + Send + Sync>`, and its own documentation says it is called once during Worker registration. This implementation now calls it once for registry validation and then lets Worker call it again during `flush_pending`. A non-idempotent or stateful factory can return `ToolMeta.name = "Checked"` during registry validation and `ToolMeta.name = "ActuallyRegistered"` during Worker registration. In that case capability checks, duplicate checks, skipped/report entries, and installed tool reports are keyed to the first name, while the model-visible Worker tool is registered under the second name.

This keeps the original boundary hole open for the actual Worker queueing path. It is also a behavior risk for factories whose construction has side effects, since the first materialized tool instance is discarded.

Before merge, the registry should make the validated tool identity the same materialization that Worker registers. Acceptable shapes include:

- materialize `(ToolMeta, Arc<dyn Tool>)` once in the feature registry, validate `ToolMeta.name`, then queue a stable factory that returns the already validated metadata/tool instance; or
- change the lower Worker/tool registration path to accept a materialized tool registration type; or
- otherwise enforce a type-level/single-materialization invariant so the registry-checked name cannot differ from the Worker-registered name.

Add a regression test with a `ToolDefinition` that returns different names across calls; it should not be possible for the report to say one name while the Worker registers another.

### Prior blocker 2: Capability grants for non-tool/hook surfaces

Status: **resolved for this Phase 1/2 slice**.

The fix adds capability enforcement or explicit denial/skipping for the previously uncovered surfaces:

- `BackgroundTaskRegistrar::declare` now checks `DeclareBackgroundTask` through `require_background_task_capability`.
- Descriptor-declared background tasks are also checked before being recorded in `declared_background_tasks`.
- `FeatureServiceRegistrar::provide` now checks `ProvideService` through `require_service_provider_capability`.
- Descriptor-declared service providers are also checked before registration in `FeatureServiceRegistry`.
- Service requirements are checked against `RequireService`; required missing/denied requirements block installation, while optional missing/denied requirements are skipped/degraded.
- `FeatureNotificationSink`, `FeatureAlertSink`, and `FeatureDiagnosticSink` now require `EmitNotification`, `EmitAlert`, and `EmitDiagnostic` respectively before recording their skeleton/report-only output.

The install path still uses `CapabilityGrantSet::grant_all(&descriptor.requested_capabilities)`, so this slice is not a real host policy resolver yet. That is acceptable for the focused registry skeleton as long as absent/unrequested capabilities are denied/skipped, which the updated code now does.

## 4. Blockers

### Blocker — ToolDefinition is validated once but registered from a second factory call

The registry must not validate/report one tool name while Worker can register another. The current fix validates one call to the `ToolDefinition` but queues the same factory for a later Worker call, so `ToolMeta.name` can still diverge between registry checks and actual model-visible registration.

This must be fixed before merge because it directly affects the requested invariant: capability checks, duplicate checks, skipped/reports, and Worker queueing must share the same model-visible tool identity.

## 5. Non-blockers / follow-ups

- The new tests cover the stable-factory mismatch case, background-task capability denial, service-provider capability denial, service requirement basics, duplicate tool names, and Task tool registration. They do not yet cover notification/alert/diagnostic sink capability denial. The code path is straightforward, so I do not classify this as a blocker, but adding a small sink-denial test would make the capability-surface coverage more complete.
- The service resolution remains installation-order based. This was already noted in the original review and remains acceptable as a follow-up for this descriptor/skeleton slice.
- The install report is still not surfaced outside local `_feature_install_report` plumbing. This remains acceptable for the small built-in proof but should be addressed when discovery/enablement diagnostics become user-visible.

## 6. Validation assessed or rerun

Reran:

- `git diff --check develop...HEAD` — passed.
- `cargo test -p pod feature::tests --lib` — passed: 7 tests, 0 failures.

Assessed by inspection:

- Original review artifact.
- Ticket item and delegation intent.
- `git diff a8ae6ca..HEAD -- crates/pod/src/feature.rs`.
- Current `crates/pod/src/feature.rs` around tool registration, capability gates, service/background handling, notification/alert/diagnostic sinks, and tests.
- Search for generic event/UI/context/history surfaces in `crates/pod/src/feature.rs`.

I did not rerun full `cargo test -p pod --lib`, workspace check, or `nix build .#yoi` in this rereview because the remaining issue is visible by focused inspection and the requested scope was blocker re-review.

## 7. Residual risk

No new generic event channel, custom UI/dialog path, hidden context/history injection, raw `Pod`, raw `ToolServerHandle`, raw `Interceptor`, raw history writer, raw event sender, or raw `NotifyBuffer` exposure was found in the feature API changes. Once the tool materialization/registration identity issue is fixed, the remaining risk looks appropriate for the intended descriptor-first Phase 1/2 slice.

---
