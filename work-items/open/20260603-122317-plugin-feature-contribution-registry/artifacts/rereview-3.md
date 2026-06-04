# Rereview 3: plugin-feature-contribution-registry

## 1. Result: approve

Approve. The remaining descriptor/contribution boundary blocker from `rereview-2.md` is fixed in `6fa08f8 fix: enforce feature descriptor contributions`, and I did not find new blockers in the final review scope.

## 2. Summary of implementation

The final fix adds descriptor-derived contribution membership checks in `crates/pod/src/feature.rs`:

- `FeatureContributionDeclarations` is built from each `FeatureDescriptor` before install.
- `FeatureInstallContext` passes those descriptor-approved sets to the typed registrars.
- Tool, Hook, BackgroundTask, and ServiceProvider install-time contributions are rejected with `FeatureInstallError::UndeclaredContribution` and a skipped contribution report entry when they are not present in the descriptor-approved set.
- The previous once-materialized tool registration hardening remains intact: the materialized `ToolMeta.name` is still the identity used for descriptor membership, duplicate checking, install reporting, and Worker registration.

The implementation remains a first Pod-layer feature registry slice: task tools are the migrated builtin proof, services/background tasks are still descriptor/report skeletons, and external plugin discovery/loading is not implemented.

## 3. Requirement-by-requirement assessment

### Remaining blocker: descriptor-approved contribution set enforcement

Status: fixed.

- Descriptor contribution sets are derived in `FeatureContributionDeclarations::from_descriptor` (`feature.rs:582-633`) for:
  - tools by model-visible name;
  - hooks by `(name, FeatureHookPoint)`;
  - background tasks by task name;
  - service providers by `(ServiceId, version)`.
- The install loop constructs those declarations per descriptor and passes them through `FeatureInstallContext` (`feature.rs:1104-1196`).
- Undeclared contributions are recorded through `reject_undeclared_contribution`, which marks the contribution as skipped and returns `FeatureInstallError::UndeclaredContribution` (`feature.rs:635-649`, `feature.rs:1217-1244`).

### Tools: undeclared rejection and once-materialized identity

Status: fixed.

- `ToolContributionRegistrar::register` materializes the `ToolDefinition` exactly once at registration time (`feature.rs:749-751`).
- It compares the explicit contribution name with the materialized model-visible `ToolMeta.name` (`feature.rs:752-763`).
- It checks descriptor membership using that same materialized `model_visible_name` before authority or duplicate checks (`feature.rs:765-772`).
- Duplicate checks and install reporting still use the same `model_visible_name` (`feature.rs:784-800`).
- Worker registration receives a frozen closure over the already-materialized `tool_meta` and `tool` (`feature.rs:801-802`), so a stateful/non-idempotent `ToolDefinition` cannot register a different name after checks.
- `undeclared_tool_contribution_is_rejected` verifies an undeclared tool is not queued into `pending_tools`, is not marked installed, and is reported/skipped (`feature.rs:1615-1640`).
- `stateful_tool_definition_is_materialized_once_for_report_and_worker` still covers the previous materialization regression (`feature.rs:1466-1519`).

### Hooks: `(name, hook point)` descriptor check

Status: fixed.

- `HookContributionRegistrar::require_declared` checks the descriptor-approved `(name, FeatureHookPoint)` before adding any hook to `HookRegistryBuilder` (`feature.rs:815-829`).
- Each hook registrar method calls `require_declared` before installing the hook (`feature.rs:831-877`).
- `undeclared_hook_contribution_is_rejected` verifies the feature is not installed, `installed_hooks` remains empty, and a skipped Hook contribution is reported (`feature.rs:1642-1661`).

### Background tasks

Status: fixed for this descriptor/report-only slice.

- `BackgroundTaskRegistrar::declare` rejects undeclared task names before adding to the report (`feature.rs:887-909`).
- `undeclared_background_task_contribution_is_rejected` verifies the feature is not installed, no background task is reported as declared, and a skipped BackgroundTask contribution is recorded (`feature.rs:1663-1683`).
- Declared background tasks remain descriptor/report contributions and are not modeled as sandbox authorities.

### Service providers

Status: fixed for this descriptor/report-only slice.

- `FeatureServiceRegistrar::provide` rejects undeclared `(ServiceId, version)` provider declarations before registering them with `FeatureServiceRegistry` (`feature.rs:920-941`).
- `undeclared_service_provider_contribution_is_rejected` verifies the feature is not installed, the service registry does not provide the hidden service, the report has no provided service, and a skipped Service contribution is recorded (`feature.rs:1685-1706`).
- Declared service providers remain descriptor/report metadata in this slice; no broad service handle or raw provider API is exposed.

### No contribution-capability variants reintroduced

Status: passes.

Focused searches found no reintroduced public feature authority variants such as `ContributeTool`, `ContributeHook`, `DeclareBackgroundTask`, `ProvideService`, `RequireService`, `EmitAlert`, `EmitDiagnostic`, `RunBackgroundTask`, `FeatureCapability`, or `HostCapability`.

`HostAuthority` remains limited to host APIs/handles: filesystem, network, secret refs, model notification, Pod management, state store, and service access (`feature.rs:81-89`). This matches the permission-boundary decision: contributions are descriptor/digest-locked declarations, while host authorities represent dangerous host access.

### No generic event channel, UI/dialog protocol, hidden history/context injection, or raw internal handles

Status: passes.

I found no new generic plugin event channel, custom UI/dialog protocol, hidden context/history injection path, or broad unrelated refactor. `FeatureInstallContext` still exposes typed registrars/sinks only, not raw `Pod`, `ToolServerHandle`, `Interceptor`, raw history writer, raw event sender, or raw `NotifyBuffer`.

`FeatureNotificationSink::notify_model` remains gated by `HostAuthority::ModelNotification` and only records a skipped diagnostic because no durable Notify/SystemItem host is attached during install (`feature.rs:667-693`). Alert and diagnostic sinks remain bounded install-report style surfaces, not generic UI/event channels (`feature.rs:696-736`).

### Task tools behavior and normal PreToolCall path

Status: passes.

- Core builtin tools are still registered directly through `core_builtin_tools`; task tools are installed via the feature registry (`controller.rs:517-526`).
- The task feature uses existing `tools::task_tools` definitions, so schemas and execution behavior remain owned by the existing task tool implementations.
- `builtin_task_feature_installs_through_worker_tool_path` verifies Worker-visible task tool names are unchanged: `TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate` (`feature.rs:1800-1825`).
- Because task tools still register into the normal Worker tool registry, model calls continue through the existing tool execution and PreToolCall permission path.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- Keep `AuthorityGrantSet::grant_all(...)` scoped to this builtin-only scaffold. Before enabling external/untrusted feature sources, add the real host policy/user approval grant resolver.
- The descriptor membership checks currently lock tool contributions by model-visible tool name. If future external-plugin approval wants to display/lock exact model-visible tool schema/description too, extend `ToolDeclaration` and tests accordingly.
- Add explicit size/rate/secrecy bounds for feature diagnostic and alert strings before exposing these sinks to untrusted plugins.
- Clarify future semantics for partially installed features if a module registers one approved contribution and then fails on a later contribution. The current slice prevents undeclared contributions from being installed, but approved earlier contributions can remain queued before the install error is reported.

## 6. Validation assessed or rerun

Read/assessed:

- `rereview-2.md`, ticket, and permission-boundary decision;
- final `6fa08f8` changes in `crates/pod/src/feature.rs`;
- task tool registration path in `crates/pod/src/controller.rs`, `crates/pod/src/pod.rs`, and `crates/tools/src/lib.rs`;
- focused searches for removed contribution-capability names and raw internal exposure terms.

Commands run from `/home/hare/Projects/yoi/.worktree/plugin-feature-contribution-registry`:

- `git status --short`
- `git log --oneline --decorate -5`
- `git diff --name-status develop...HEAD`
- `git diff --check develop...HEAD`
- `git show --stat --oneline --decorate --no-renames 6fa08f8`
- `git show --name-only --format='%H %s' 6fa08f8`
- focused grep/search/read checks over the changed feature API and registration paths

`git diff --check develop...HEAD` exited successfully. I did not rerun cargo or Nix builds in this review turn because the available write scope is limited to the review artifact path and those validations would write build outputs.

## 7. Residual risk

Residual risk is acceptable for this slice. The registry is still a skeleton for external plugins: background tasks, services, notification sinks, alert sinks, and authority grant resolution are represented but not fully connected to production host lifecycles or approval policy. That is consistent with the ticket’s first-slice scope as long as follow-up work does not treat this as external-plugin-ready without adding the real host policy, lifecycle, and bounded-output enforcement.
