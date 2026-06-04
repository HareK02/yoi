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
