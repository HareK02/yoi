# Public Pod-side API for Feature / Plugin Contributions

## 1. Summary recommendation

Introduce a `pod::feature` public API as the single Pod-side registration layer for built-in features and future external plugins. A feature module should declare its identity, contributions, service dependencies/exports, and requested host authorities. Contributions are displayed and locked by descriptor/digest; host authorities are the user-approved sandbox/object-capability grants such as filesystem, network, secrets, Pod management, and model-visible durable notification/history append.

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
  - Feature tools must remain subject to the same PreToolCall permission path. Feature authority grants do not replace per-call tool permission.

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
    pub mod background;
    pub mod capability;
    pub mod diagnostic;
    pub mod hook;
    pub mod notify;
    pub mod registry;
    pub mod service;
    pub mod tool;

    pub use background::{BackgroundTaskContribution, BackgroundTaskRegistrar};
    pub use capability::{AuthorityGrantSet, AuthorityRequest, HostAuthority};
    pub use diagnostic::{FeatureDiagnostic, FeatureInstallReport};
    pub use notify::{FeatureAlertSink, FeatureNotifySink};
    pub use registry::{FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureModule, FeatureRuntimeKind};
    pub use service::{FeatureServiceProvider, FeatureServiceRegistrar, ServiceDeclaration, ServiceRequirement, YoiService};
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
    pub requested_authorities: Vec<AuthorityRequest>,
    pub provides_services: Vec<ServiceDeclaration>,
    pub requires_services: Vec<ServiceRequirement>,
    pub declared_tools: Vec<ToolDeclaration>,
    pub declared_hooks: Vec<HookDeclaration>,
    pub declared_background_tasks: Vec<BackgroundTaskDeclaration>,
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
    pub grants: &'a AuthorityGrantSet,
    pub tools: ToolRegistrar<'a>,
    pub hooks: PublicHookRegistrar<'a>,
    pub services: FeatureServiceProvider<'a>,
    pub service_exports: FeatureServiceRegistrar<'a>,
    pub background_tasks: BackgroundTaskRegistrar<'a>,
    pub notify: FeatureNotifySink<'a>,
    pub alerts: FeatureAlertSink<'a>,
    pub diagnostics: FeatureDiagnosticSink<'a>,
}
```

Important details:

- `FeatureDescriptor` is declarative and serializable. It is safe to show in diagnostics, profile previews, and `ListFeatures`-style future tooling.
- `FeatureModule::install` is runtime code that wires stateful tool/hook implementations into host registrars.
- `FeatureInstallContext` must not expose `Pod`, `Worker`, raw `ToolServerHandle`, raw `Interceptor`, raw `NotifyBuffer`, raw `LogWriter`, raw `event_tx`, or direct history mutation.
- `FeatureServiceProvider` returns only host-mediated services backed by granted host authorities and resolved service dependencies, for example scoped filesystem access, WorkItem store access, memory access, Pod orchestration handles, web provider handles, secret references, or another feature's declared public service. It should return `Denied`/`Unavailable` diagnostics instead of exposing partial internals.
- `FeatureServiceRegistrar` lets a feature expose a narrow public service API to other features. This is an extension boundary for future plugin-to-plugin APIs, not a requirement to move already-implemented core behavior out of the host.

### Example registration snippet

This is illustrative shape, not proposed final exact Rust syntax:

```rust
use pod::feature::{
    AuthorityRequest, FeatureDescriptor, FeatureId, FeatureInstallContext,
    FeatureModule, FeatureRuntimeKind, HostAuthority, ToolContribution,
};

pub struct WorkItemFeature {
    state: std::sync::Arc<WorkItemFeatureState>,
}

impl FeatureModule for WorkItemFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builder(FeatureId::builtin("work-item"))
            .display_name("WorkItem intake and routing")
            .runtime(FeatureRuntimeKind::Builtin)
            .request(AuthorityRequest::required(
                HostAuthority::WorkItemStore { read: true, write: true },
                "create and update WorkItem records through host-owned ticket storage",
            ))
            .request(AuthorityRequest::optional(
                HostAuthority::ModelNotification,
                "commit user-action-required notices through the existing Notify/SystemItem path when the model should see them",
            ))
            .request(AuthorityRequest::optional(
                HostAuthority::Alert,
                "surface short transient human-facing warnings when the watcher fails",
            ))
            .tool("WorkItemCreate")
            .tool("WorkItemComment")
            .hook("work_item_intake_pre_tool_audit", pod::hook::HookPoint::PreToolCall)
            .background_task("work_item_watch")
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

        ctx.background_tasks.register(BackgroundTaskContribution::pod_lifetime(
            "work_item_watch",
            WorkItemWatchTask::new(store.clone(), self.state.clone()),
        ))?;
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
}
```

Rules:

- Register into the existing `ToolRegistry` / `ToolServerHandle`; do not add a plugin-dispatcher tool that multiplexes plugin calls outside normal tool history.
- Preserve normal `PreToolCall` permission evaluation, tool-call history, result history, output truncation/bounding, and diagnostic behavior.
- Host-controlled feature enablement decides whether a contributed tool is installed. Manifest/profile tool permission still decides whether a model may call it at runtime.
- Duplicate tool names should be rejected during feature registry preflight with a diagnostic, not discovered later through a panic or undefined ordering.
- Public feature identity should be source-qualified (`builtin:memory`, `project:foo`, `plugin:<digest>:bar`), while model-visible tool names should remain explicit stable names. Do not auto-prefix model tool names unless the project deliberately chooses a future namespacing policy.
- Tool schemas/descriptions must be part of the normal `ToolDefinition` path so model-visible surfaces remain inspectable and bounded.
- If a required host authority or service is not granted or configured, the tool should not be registered; the install report should explain the skipped contribution.

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

### Notification, alert, and diagnostic surfaces

Do not add a plugin-defined event-channel API. A feature should not be able to publish arbitrary structured UI payloads or ask clients to render feature-specific dialogs through the base Pod API.

Expose narrowly-scoped host sinks instead:

```rust
pub struct FeatureNotifySink<'a> { /* host-owned */ }
pub struct FeatureAlertSink<'a> { /* host-owned */ }
pub struct FeatureDiagnosticSink<'a> { /* host-owned */ }
```

Recommended behavior:

- `FeatureNotifySink::notify_model(...)` is the feature-facing form of the existing `Method::Notify` / `SystemItem::Notification` path. It creates a model-visible, history-backed notification by asking the host to commit a `LogEntry::SystemItem`; clients then see the same committed item through `Event::SystemItem`. Disk-side and wire-side remain 1:1.
- `FeatureAlertSink::alert(...)` emits a short human-facing transient alert equivalent to `Event::Alert`. It is UI/person-facing only: not model-visible, not session history, not replayed as an input, and not a structured UI extension channel.
- `FeatureDiagnosticSink` records install/runtime/authority/task diagnostics for host logs, future `ListFeatures`, profile validation, and TUI diagnostics. Diagnostics are host-defined records, not arbitrary client-rendered UI payloads.
- Dialogs, confirmations, and other interactive client UI are out of scope for the base API. If needed later, add a separate host-defined interaction protocol; do not use a generic event channel for it.
- Model-visible notifications require a host authority grant because they become durable model/history-visible input. Transient alerts and diagnostics are host-defined reporting surfaces; normally control them with descriptor approval, host policy, size bounds, and rate limits rather than treating them as sandbox authorities.
- Background feature tasks must use these sinks; they must not hold raw log writers, raw event senders, or append directly to history.

### Background task contribution

Background tasks should be a first-class feature contribution. Without this, asynchronous features will inevitably create ad hoc `tokio::spawn` loops, private channels, and shutdown/reporting paths outside the registry.

Initial shape:

```rust
pub struct BackgroundTaskContribution {
    pub feature_id: FeatureId,
    pub id: FeatureTaskId,
    pub description: String,
    pub activation: TaskActivation,
    pub restart: RestartPolicy,
    pub shutdown: ShutdownPolicy,
    pub requested_authorities: Vec<AuthorityRequest>,
}

pub struct FeatureTaskContext<'a> {
    pub cancellation: CancellationToken,
    pub notify: FeatureNotifySink<'a>,
    pub alerts: FeatureAlertSink<'a>,
    pub diagnostics: FeatureDiagnosticSink<'a>,
    pub services: FeatureServiceProvider<'a>,
}
```

Rules:

- The host starts, tracks, cancels, and reports background tasks. Feature modules register tasks; they do not spawn untracked runtime loops.
- Task output is limited to granted authorities, sinks, and services: model-visible notification through `FeatureNotifySink` only with a model-notification authority grant, human transient alert through `FeatureAlertSink`, diagnostics through `FeatureDiagnosticSink`, and host-granted service operations.
- The host records task lifecycle/status in install/runtime diagnostics. Do not expose arbitrary feature-defined event channels for task progress.
- Initial activation can be conservative (`PodLifetime` and/or `OnDemand`); restart policy may start with `Never`, but the API should make shutdown/cancellation behavior explicit from the beginning.

### Service contribution and dependency

Add a host-mediated service registry so a feature/plugin can publish a narrow API and another feature/plugin can depend on that API without importing the provider's concrete implementation or bypassing authority policy.

This is not a plan to move existing implemented core features out of the host immediately. Core-backed APIs may remain core-backed. The service form exists so future detachable features can depend on stable public interfaces rather than private Pod internals or another plugin's concrete state.

Initial shape:

```rust
pub trait YoiService: Send + Sync + 'static {
    fn service_id(&self) -> ServiceId;
    fn service_version(&self) -> ServiceVersion;
}

pub struct ServiceDeclaration {
    pub id: ServiceId,                  // e.g. yoi.memory.v1
    pub version: ServiceVersion,
    pub description: String,
    pub operations: Vec<ServiceOperationDeclaration>,
}

pub struct ServiceRequirement {
    pub id: ServiceId,
    pub version: ServiceVersionReq,
    pub optional: bool,
    pub reason: String,
}

pub struct FeatureServiceRegistrar<'a> { /* host-owned */ }
pub struct FeatureServiceProvider<'a> { /* host-owned */ }
```

Rules:

- A feature may provide a service through `FeatureServiceRegistrar`, and consumers may obtain the service only through `FeatureServiceProvider` after host dependency resolution and authority grant checks.
- Features must not directly hold another feature's concrete module/state unless that handle was returned by the host service registry.
- Service identity is source-independent interface identity (`yoi.memory.v1`, `yoi.pod-management.v1`, `project.issue-tracker.v1`), while provider identity remains a `FeatureId`.
- Service dependencies are resolved during feature registry preflight. Required missing services skip the consumer feature with diagnostics; optional missing services install the consumer in a degraded mode if its installer supports that.
- Service dependency cycles are rejected initially. Late-bound/cyclic service handles are out of scope until a real need appears.
- In-process built-in services may use Rust trait objects internally. External plugin/WASM/MCP services should be represented by host-side proxies that implement the same service interface boundary; do not expose raw foreign runtime handles.
- Service handles should be authority-bound. Prefer narrowed interfaces such as read-only vs write-capable services, or enforce caller/grant checks through a host-provided service call context.

Examples:

- `builtin:memory` may provide `yoi.memory.v1` and contribute memory tools. A WorkItem intake feature may require `yoi.memory.v1` optionally for contextual lookup.
- `builtin:pod-orchestration` may provide `yoi.pod-management.v1` and contribute Pod management tools. The actual Pod lifecycle/scope authority remains host-owned; the service is a controlled façade.
- A future issue-tracker plugin may provide `project.issue-tracker.v1`, while WorkItem tooling consumes that service without knowing the concrete plugin package.

### Host authority request/grant/diagnostics

Authority grants are the user-approved sandbox/object-capability boundary. They control which host-provided APIs or handles a feature receives. They are not a mirror of every contribution a feature declares.

Contributions such as tools, hooks, background tasks, and service providers should be displayed and locked by descriptor/digest at install time. They do not need separate `ContributeTool` / `ContributeHook` / `RunBackgroundTask` / `ProvideService` authority variants. If a plugin changes its contributed tools/hooks/tasks/services, the descriptor or package hash changes and the user must re-approve the changed plugin.

Initial authority categories:

```rust
pub enum HostAuthority {
    Fs(FsGrant),
    Network(NetworkGrant),
    SecretRef { id: String },
    ModelNotification,              // durable Notify/SystemItem/Event::SystemItem path
    WorkItemStore { read: bool, write: bool },
    MemoryStore { read: bool, write: bool },
    PodManagement { spawn: bool, message: bool, restore: bool },
    PersistentPluginState { read: bool, write: bool },
}
```

Important separation:

- Descriptor/contribution approval decides whether a feature with this digest may be installed with its declared tools/hooks/tasks/services.
- Authority grants decide whether a feature may receive dangerous host handles such as filesystem, network, secret, Pod-management, store, or model-visible notification access.
- Tool permissions decide whether an installed model-visible tool call may execute for a specific Pod/run.
- Scope permissions decide which filesystem paths or delegated Pod capabilities a host service may touch.

Diagnostics should be first-class:

```rust
pub struct FeatureInstallReport {
    pub feature_id: FeatureId,
    pub enabled: bool,
    pub granted: Vec<HostAuthority>,
    pub denied: Vec<AuthorityDenial>,
    pub installed_tools: Vec<ToolName>,
    pub installed_hooks: Vec<String>,
    pub installed_background_tasks: Vec<FeatureTaskId>,
    pub provided_services: Vec<ServiceId>,
    pub resolved_services: Vec<ServiceResolutionReport>,
    pub skipped_contributions: Vec<SkippedContribution>,
    pub diagnostics: Vec<FeatureDiagnostic>,
}
```

Diagnostics must avoid secrets and must be safe for session logs, TUI display, and future `ListFeatures`/profile validation output.

## 4. State ownership model

Feature state belongs to the feature module.

- A feature may own `Arc<State>` and clone it into contributed tools, hooks, background tasks, and service implementations.
- The Pod registry stores descriptors, service resolution records, install reports, enabled/disabled status, and host-owned handles. It does not store feature business state.
- Durable feature data must live in a feature-owned or host-granted store with an explicit API: WorkItem files through a WorkItem service, memory records through memory APIs, plugin config/state through a future plugin-state service, etc.
- Session history is not feature storage. It is an audit/replay record of model-visible interactions and committed system items.
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
- Grant themselves host authorities or infer grants from successful construction.
- Mutate manifest/profile/scope state directly.
- Perform filesystem/process/network/secret access outside granted host services.
- Depend on another feature/plugin by concrete implementation, private state, raw process handle, or direct module pointer instead of a host-resolved service interface.
- Emit unbounded tool outputs, diagnostics, alerts, task-status reports, or notification bodies.
- Emit arbitrary plugin-defined UI payloads, dialogs, or event-channel messages.
- Put secrets into diagnostics, session logs, model context, TUI output, or feature install reports.
- Depend on MCP/WASM/package-distribution mechanics in the base Pod API.

Positive invariant: if the model can see a feature-produced fact, a future replay/resume must have a durable explanation for why that fact was present.

## 6. Placement and crate-boundary recommendation

Recommended placement:

- `crates/pod/src/feature.rs` or `crates/pod/src/feature/mod.rs`
  - public feature traits/types
  - feature registry builder
  - install reports/diagnostics
  - authority request/grant model
  - typed registrars/sinks
  - service registry, service declarations, and dependency resolution reports

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

1. Resolve manifest/profile and host authority policy.
2. Construct `Pod` and internal safety surfaces.
3. Install host/internal hooks such as manifest permission enforcement.
4. Build enabled feature descriptors, collect declared service providers/requirements, resolve the service dependency DAG, and compute authority grants.
5. Install enabled feature modules through `FeatureRegistryBuilder`, including service exports, tools, hooks, and background task declarations.
6. Flush/register tools through the existing Worker tool registry.
7. Freeze/install the Pod interceptor, start host-managed background tasks at their activation point, and start normal run/attach behavior.

The exact sequencing can be adjusted to match current construction, but the invariant should hold: public feature hooks cannot precede host safety hooks, and feature tools must exist before the model receives the final tool schema for a run.

## 7. Migration path from current built-in registrations

Recommended migration is incremental and behavior-preserving:

1. Land hook public-surface hardening first.
   - Remove/replace public raw `Item`-carrying hook actions.
   - Define which hook decisions are safe for external contributors.

2. Add `pod::feature` with no behavior change.
   - Implement descriptors, service declarations/requirements, authority grants, install reports, and registrars.
   - Initially register no external plugins.

3. Add the service registry as a host-mediated boundary.
   - Start with core-backed services or a trivial built-in service provider to validate provider/consumer resolution.
   - Do not move existing Memory or Pod management implementation solely for this ticket.
   - Use diagnostics for missing required services, optional degraded service dependencies, and service cycles.

4. Wrap current built-in tool registration as built-in feature modules.
   - Start with a small built-in feature whose state/services are already cleanly bounded.
   - Preserve existing tool names, schemas, and permission behavior.
   - Convert duplicate-name failures into registry diagnostics before flushing tools.

5. Move larger built-in groups behind feature modules.
   - Filesystem/process tools from `crates/tools`.
   - Memory tools.
   - Pod orchestration tools.
   - Task/WorkItem tools once their stores and hooks have explicit capabilities.
   - Web tools as configured provider-backed features.

6. Move built-in hook contributions only after safe hook semantics are stable.
   - Keep manifest permission enforcement as an internal host hook, not a feature hook.
   - Keep accounting/usage hooks internal unless they become genuine feature behavior.

7. Treat workflow/user-input expansion separately.
   - Workflow invocation already uses a durable system-item attachment pattern.
   - Do not expose arbitrary workflow-like context injection to plugins until there is a safe typed command/input-contribution API with durable append semantics.

8. Add profile/manifest enablement after built-ins work through the same registry.
   - Built-ins and external plugins should share descriptor/authority/install-report mechanics.
   - Host policy may grant built-ins by default, but built-ins should still declare what they use.

## 8. Impact on WorkItem / MCP / plugin distribution follow-ups

WorkItem / intake routing:

- WorkItem routing can become a built-in feature that contributes WorkItem tools, optional routing hooks, and host-managed background tasks such as a watcher.
- It should request `WorkItemStore` and model-notification authority instead of reaching into ticket files or prompt context ad hoc. Its background task is a declared contribution, not a separate sandbox authority.
- Model-visible routing hints or intake results must be committed through notification/history append paths.
- This registry gives the WorkItem feature a clean way to install without making WorkItem a special Pod runtime mode.
- If WorkItem, Memory, or Pod orchestration are split into smaller feature modules later, they should communicate through declared services such as `yoi.work-item-store.v1`, `yoi.memory.v1`, or `yoi.pod-management.v1` rather than private module references.

Feature-to-feature service APIs:

- The service registry lets plugins expose stable APIs without requiring the host to make every domain service a permanent core API.
- This does not force existing Memory or Pod management implementations to be extracted immediately. They may stay core-backed while still being representable as service façades for consumers.
- External plugins should consume service proxies provided by the host, not another plugin's raw process/WASM/MCP handle.

MCP:

- MCP should be an adapter/runtime kind that produces normal `ToolContribution`s and, when needed, host-managed background tasks for connection/session supervision plus bounded diagnostics/alerts.
- MCP tool calls must still pass through `ToolRegistry`, PreToolCall permission, output bounding, and history result recording.
- MCP resources/prompts should not become invisible prompt injection. If exposed later, they should be explicit tools, user-invoked attachments, or durable notification/history appends.
- MCP transport/session details are out of scope for the base API beyond the `FeatureRuntimeKind::McpBridge` placeholder.

Plugin distribution:

- Archive validation, cache extraction, signing/trust, WASM execution, external process supervision, and package update policy should remain separate follow-up designs.
- Distribution mechanisms should eventually produce the same descriptor/authority/contribution objects as built-ins.
- Authority grants are the host trust boundary; package installation alone must not grant runtime authority.

## 9. Open questions / risks

1. Tool naming policy is the highest-risk API decision.
   - Recommendation: feature identities are source-qualified, model-visible tool names stay explicit and stable, and collisions are rejected by the host.
   - Risk: external plugins may need namespacing later. Auto-prefixing now would avoid collisions but would also change model-facing ergonomics and diverge from current built-in tool names.

2. The exact safe hook action set must be settled by `hook-public-surface-hardening`.
   - Especially important: whether public pre-tool hooks may synthesize denials/results, and how durable append requests are represented.

3. Notification, alert, and diagnostic semantics need precise names and capabilities.
   - Model-visible notifications must be durable and should use the existing `Notify` / `SystemItem` / `Event::SystemItem` path.
   - `Alert`-like output is transient human-facing text only.
   - Diagnostics/status are host-defined operational records, not plugin-defined UI channels.

4. Authority granularity can easily become either too coarse or too noisy.
   - Start with coarse host authorities plus normal tool permissions, then split only when real features need finer grants.

5. Runtime enable/disable is not designed here.
   - Initial registry should be install-at-startup. Hot reload or dynamic plugin enablement needs separate lifecycle, cleanup, and schema-refresh design.

6. Persistent plugin state needs a future host service.
   - The base API says state is feature-owned, but external plugins will still need a sanctioned durable state directory/store with migration/versioning rules.

7. Background tasks need lifecycle policy.
   - If external plugins can spawn tasks, the host must define shutdown, cancellation, panic handling, diagnostic routing, and whether task output may become model-visible.

8. Service API design needs an in-process vs external-plugin boundary.
   - Built-in services can use Rust traits or typed handles, but external plugins need host-side proxies and operation schemas. The first implementation should avoid baking in Rust trait-object assumptions as the only service representation.
   - Service handles must be authority-bound, or one broad service handle can accidentally become an authority escalation path.

9. Existing workflow/input expansion is close to the forbidden boundary.
   - It is safe only because it commits system items before model visibility. Any future plugin command/input contribution must preserve that durable replay property.
