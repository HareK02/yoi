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