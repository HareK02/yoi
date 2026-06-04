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