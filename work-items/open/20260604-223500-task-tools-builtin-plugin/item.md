---
id: 20260604-223500-task-tools-builtin-plugin
slug: task-tools-builtin-plugin
title: Feature: extract Task tools as builtin module
status: open
kind: task
priority: P1
labels: [plugin, feature-registry, tasks]
created_at: 2026-06-04T22:35:00Z
updated_at: 2026-06-04T23:50:50Z
assignee: null
legacy_ticket: null
---

## Issue

The feature contribution registry slice proved the registry path by registering `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` through a built-in `FeatureModule`. That is a useful proof, but the Task tool group still lives as an inline built-in helper inside the registry implementation rather than as a clean internal module.

As a trial of the Feature API boundary, extract the Task tool group into a first-class built-in internal module using the same public-ish Feature API shape that future built-in modules and external plugins should use. This should validate whether the registry API is usable without giving Task tools special Pod-side wiring or treating internal module extraction as an external-plugin permission problem.

## Direction

Treat Task tools as a built-in feature/internal module, not as an ad hoc registration branch.

- The module contributes only the existing Task tools:
  - `TaskCreate`
  - `TaskUpdate`
  - `TaskGet`
  - `TaskList`
- The module owns or receives the existing `TaskStore` handle from the Pod host.
- Tool names, schemas, descriptions, behavior, task reminder/compaction behavior, and model-visible history behavior must not change.
- Task tools remain subject to the normal ToolRegistry and PreToolCall permission path.
- Contributions are descriptor-declared and registry-validated; no contribution-capability gates are reintroduced.
- No external plugin loading, package format, WASM, MCP, WorkItem, or UI/dialog system is in scope.

## Requirements

- Move the Task built-in feature out of the generic registry implementation into a clean module boundary suitable as a reference pattern for built-in feature modules.
  - Candidate placement: `crates/pod/src/feature/builtin/task.rs` or equivalent.
  - Keep `pod::feature` focused on registry/types rather than concrete built-in feature implementations.
- Expose a clear construction function such as `task_tools_feature(task_store: tools::TaskStore) -> impl FeatureModule` from the built-in feature module.
- Preserve the existing TaskStore sharing semantics:
  - one Pod-lifetime/session TaskStore shared by all four Task tools;
  - current TaskStore snapshot/reminder/compaction behavior remains valid.
- Ensure the Task feature descriptor exactly declares the four Task tools and no host authorities.
- Keep descriptor-approved contribution checks active for Task tools.
- Keep once-materialized tool identity behavior from the registry slice.
- Add/update focused tests proving:
  - Task tools install through the built-in feature module;
  - descriptor declarations match the installed tool names;
  - normal TaskCreate/TaskUpdate/TaskGet/TaskList behavior still works or existing tests continue to cover it;
  - no contribution authority variants are reintroduced.
- Document, in code comments or test names where appropriate, that this is the reference built-in feature module pattern.

## Non-goals

- External plugin discovery/loading.
- Plugin package format or descriptor lock files.
- WASM/runtime sandboxing.
- WorkItem/MCP integration.
- Moving TaskStore persistence/lifecycle semantics.
- Changing Task tool names/schemas/descriptions.
- Resolving the broader internal-module vs external-plugin authority split; that is tracked separately by `feature-api-authority-separation`.
- Adding/removing Task tools.
- Reworking all built-in tool groups.

## Acceptance criteria

- Task tool registration is represented as a clean built-in feature module rather than inline registry implementation detail.
- Behavior and model-visible tool metadata for `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` are unchanged.
- Feature descriptor declarations and install-time contributions are reconciled through the existing registry boundary.
- No new host authority grants are required for Task tools.
- Focused tests and workspace checks pass.
