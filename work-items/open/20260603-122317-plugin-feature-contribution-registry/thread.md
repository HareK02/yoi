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
