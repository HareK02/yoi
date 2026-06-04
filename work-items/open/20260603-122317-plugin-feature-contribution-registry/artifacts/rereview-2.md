# Rereview 2: plugin-feature-contribution-registry

## 1. Result: request changes

Request changes. The prior ToolDefinition materialization blocker is fixed, and the authority enum no longer treats contribution kinds as sandbox authorities. However, the updated authority-boundary design now relies on descriptor/digest approval to control Tool/Hook/BackgroundTask/ServiceProvider contributions, and the implementation does not enforce that install-time contributions match the descriptor-declared contributions. That leaves a merge-blocking boundary gap before this registry becomes the common built-in/future-plugin contribution boundary.

## 2. Summary of implementation

Reviewed commits:

- `a8ae6ca feat: add pod feature registry slice`
- `4070176 fix: harden feature contribution gates`
- `98bbd6f fix: align feature authority boundaries`

The implementation adds `crates/pod/src/feature.rs` with:

- feature identity/runtime metadata, descriptors, contribution declarations, host authority requests/grants, install reports, diagnostics, and skipped-contribution reporting;
- `FeatureModule` / `FeatureInstallContext` / `FeatureRegistryBuilder` install mechanics;
- tool contribution registration into the normal `Worker` pending-tool path;
- hook contribution registration through `HookRegistryBuilder` and the already-safe public `pod::hook` action types;
- descriptor/report skeletons for background tasks and service provider/requirement resolution;
- model-notification, alert, and diagnostic sink skeletons;
- a migrated builtin `task_feature` proving `TaskCreate`, `TaskUpdate`, `TaskGet`, and `TaskList` registration through the registry.

The follow-up fixes changed the previous tool materialization path so `ToolContributionRegistrar::register` materializes a `ToolDefinition` once, checks the materialized `ToolMeta.name`, records/report-checks that same name, and queues a frozen closure returning the same `ToolMeta` and `Tool` instance for Worker registration.

## 3. Requirement-by-requirement assessment

### 1. Previous ToolDefinition materialization blocker

Status: fixed.

- `ToolContributionRegistrar::register` materializes the contributed `ToolDefinition` once at `feature.rs:679-681`.
- The contribution name is compared with the materialized model-visible `ToolMeta.name` at `feature.rs:681-693`.
- Duplicate checking and install reporting use that same materialized `model_visible_name` at `feature.rs:705-721`.
- Worker registration receives a frozen closure over the already-materialized `tool_meta` and `tool` at `feature.rs:722-723`, so a stateful/non-idempotent definition cannot later register a different name after validation.
- Regression coverage exists in `stateful_tool_definition_is_materialized_once_for_report_and_worker` at `feature.rs:1309-1360`; it verifies the report and Worker both see `First` and that the stateful definition was called only once.

One minor caveat remains in the builtin task migration: `TaskFeature::install` derives each contribution name by calling the task `ToolDefinition` once before passing it to `ToolContribution::new` (`feature.rs:1144-1148`), then the registrar materializes again. The task tools are stable/idempotent, and the registrar would reject a later name mismatch, so this is not the previous blocker. A future helper that constructs `ToolContribution` from a `ToolDefinition` after one materialization would make this harder to misuse.

### 2. Updated authority-boundary design

Status: mostly implemented, with one blocker on descriptor/contribution reconciliation.

Implemented correctly:

- The public `HostAuthority` variants are dangerous host handles/APIs only: filesystem, network, secret ref, model notification, Pod management, state store, and service access (`feature.rs:81-89`).
- No `ContributeTool`, `ContributeHook`, `DeclareBackgroundTask`, `ProvideService`, `RequireService`, `EmitAlert`, `EmitDiagnostic`, or similar contribution-capability variants were found in the changed Pod feature API.
- Background tasks and service providers are represented as contributions/report data, not sandbox authority grants (`feature.rs:236-260`, `feature.rs:306-353`, `feature.rs:780-804`).
- Model-visible notification remains gated on `HostAuthority::ModelNotification` via `FeatureNotificationSink::notify_model` (`feature.rs:605-623`). The current implementation still only records a skipped diagnostic because no durable Notify/SystemItem host is attached during install, so it does not introduce a hidden context/history path.
- Alert/diagnostic sinks are install-report/diagnostic surfaces, not a generic event or UI channel (`feature.rs:627-667`).

Blocking gap:

- The design says contribution approval is the descriptor/digest boundary: tools, hooks, background tasks, and service providers are displayed and locked by descriptor/digest, while authority grants are only for dangerous host handles. Because contribution-capability variants were correctly removed, descriptor enforcement becomes the control point.
- The current registrars do not enforce that install-time contributions match `FeatureDescriptor` declarations:
  - tools check only `ToolContribution.name == ToolMeta.name` and cross-feature duplicates (`feature.rs:679-724`), not that the name exists in `descriptor.tools`;
  - hooks append any runtime `HookDeclaration` to the report (`feature.rs:734-777`), not that the `(name, point)` was declared in `descriptor.hooks`;
  - background tasks append any runtime declaration (`feature.rs:785-787`), not that it was declared in `descriptor.background_tasks`;
  - services can provide any runtime `ServiceDeclaration` (`feature.rs:798-803`), not that it was declared in `descriptor.provides_services`.
- As a result, a future external plugin could present an approved descriptor/digest with one contribution set and install a different/additional tool, hook, background task, or service provider without a changed descriptor. That violates the updated authority-boundary design at the point where the implementation intentionally removed contribution authorities.

### 3. No generic event/UI/context/history/raw internals

Status: passes for this slice.

- No generic plugin event channel, custom UI/dialog protocol, or arbitrary plugin UI payload was introduced.
- The notification sink does not expose raw `NotifyBuffer`, raw `Event`, `SystemItem`, or history/context mutation; it currently records a skipped diagnostic.
- `FeatureInstallContext` exposes typed registrars/sinks only. It does not expose raw `Pod`, `ToolServerHandle`, `Interceptor`, raw history writer, raw event sender, or raw `NotifyBuffer`.
- Hook contributions use the safe `pod::hook` public action surface. The hook module keeps public hooks read-only and prevents `ContinueWith(Vec<Item>)`, arbitrary `ToolResult` construction, no-result skipping, and history/context injection.
- `feature.rs` imports `llm_worker::Worker` only for the crate-private `install_into_worker` bridge; the public install context does not hand `Worker` to feature modules.

### 4. Task tools preserve model-visible names/schemas/behavior and per-call permission path

Status: passes.

- Core builtin tools remain registered through `core_builtin_tools`; task tools are split into the new builtin feature and installed by `controller.rs:524-526` through `pod.install_features(...)`.
- `builtin_task_feature_installs_through_worker_tool_path` verifies the Worker-visible names are `TaskCreate`, `TaskGet`, `TaskList`, and `TaskUpdate` (`feature.rs:1472-1498`).
- The task feature uses existing `tools::task_tools(task_store)` definitions; no tool schema or execution implementation was rewritten in this slice.
- Tool calls still enter the normal Worker/ToolRegistry path, so existing PreToolCall permission policy remains the per-call enforcement point.

### 5. Tests/validation sufficiency

Status: not sufficient until the blocker is covered.

Good focused coverage exists for:

- descriptor/report basics;
- duplicate tool-name rejection;
- mismatched contribution name vs model-visible `ToolMeta.name` rejection;
- stateful ToolDefinition materialization once for report and Worker registration;
- service requirement resolution;
- background task and service provider not being sandbox-authority-gated;
- builtin task feature registration through the Worker tool path.

Missing coverage for the blocking boundary:

- undeclared install-time tool is rejected;
- undeclared hook `(name, point)` is rejected;
- undeclared background task declaration is rejected or reported as skipped;
- undeclared service provider declaration is rejected;
- descriptor-declared but uninstalled required contribution behavior is intentionally defined/reported, if required by the registry semantics.

## 4. Blockers

### Blocker 1: install-time contributions are not locked to descriptor-declared contributions

The registry removes contribution-capability authorities, which is correct, but then must enforce descriptor/digest approval as the contribution boundary. It currently does not. Runtime install code can register/report contributions not present in the descriptor for tools, hooks, background tasks, and service providers.

Required fix before merge:

- Carry descriptor-declared contribution sets into the install context/registrars.
- Reject or explicitly skip/report any install-time tool/hook/background-task/service-provider contribution not declared by the descriptor.
- Ensure duplicate checks and Worker registration still use the materialized model-visible tool name after declaration membership is validated.
- Add regression tests for undeclared contribution rejection across at least tools and one non-tool contribution; ideally cover all four contribution kinds because the updated authority-boundary design depends on this separation.

## 5. Non-blockers / follow-ups

- Add an ergonomic `ToolContribution` constructor/helper that materializes a `ToolDefinition` once and uses the materialized `ToolMeta.name`, so future feature authors do not repeat the `TaskFeature::install` two-call pattern.
- Before enabling non-builtin/external feature sources, replace the current `AuthorityGrantSet::grant_all(&descriptor.requested_authorities)` scaffold with an actual host policy/user-approval grant resolver. This is acceptable as a scaffold for the current builtin-only slice, but it is not a real external-plugin authority gate.
- Add explicit size/rate/secrecy bounds for feature diagnostics and alert messages before exposing these sinks to untrusted plugins. The current implementation avoids generic UI/event channels, but message strings are not bounded at this API layer.
- Consider documenting ordering/requiredness semantics for descriptor-declared but not actually installed contributions, especially hooks/background tasks/services.

## 6. Validation assessed or rerun

Read/assessed:

- ticket, delegation intent, API design, permission-boundary revision, prior review, and prior rereview artifacts;
- changed files: `crates/pod/src/feature.rs`, `crates/pod/src/controller.rs`, `crates/pod/src/pod.rs`, `crates/pod/src/lib.rs`, and `crates/tools/src/lib.rs`;
- focused searches for removed contribution-capability variant names and raw internal exposure terms.

Commands run from `/home/hare/Projects/yoi/.worktree/plugin-feature-contribution-registry`:

- `git diff --name-status develop...HEAD`
- `git diff --check develop...HEAD`
- focused `grep` searches over the changed Pod feature API and registration sites

`git diff --check develop...HEAD` exited successfully. I did not rerun `cargo test`, `cargo check`, or `nix build` because this rereview was requested as a no-source-modification review with only focused read-only validation commands; those build/test commands write target/build outputs.

## 7. Residual risk

Once the descriptor/contribution reconciliation blocker is fixed, the remaining risk is mostly staged-skeleton risk: service/background/notification/alert authorities are represented but not fully connected to host-managed lifecycles or durable hosts. That is acceptable for this first builtin-only registry slice if kept explicit in follow-up tickets and not treated as external-plugin-ready authority enforcement.
