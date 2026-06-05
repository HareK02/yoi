---
id: 20260605-025100-task-domain-in-pod-feature
slug: task-domain-in-pod-feature
title: Task: move Task domain out of tools into pod built-in feature
status: open
kind: task
priority: P1
labels: [tasks, feature-registry, crate-boundary, tools]
created_at: 2026-06-05T02:51:00Z
updated_at: 2026-06-05T03:22:31Z
assignee: null
legacy_ticket: null
---

## Issue

Task is now a built-in feature module in `pod::feature::builtin::task`, and it owns Task reminder hooks and feature lifecycle behavior. However, its domain state and tool implementations still live in `crates/tools/src/task.rs` as `tools::TaskStore`, `tools::TaskEntry`, `tools::TaskStatus`, `tools::TaskSnapshot`, and `tools::task_tools(...)`.

That split is semantically wrong: `tools` should remain a low-level built-in tool helper crate, while Task is now a stateful built-in feature with session-lifetime state, restore/rewind/snapshot behavior, hooks, and model-visible reminder policy.

Keep the next step inside the `pod` crate. Do not create a new `builtin-features` crate yet. The larger crate-boundary move can wait until a `feature-api` crate exists and external plugin APIs are better established.

## Direction

Move Task domain and Task tool implementation from `tools` into the Pod built-in Task feature module.

Target shape:

- `crates/pod/src/feature/builtin/task.rs` or a nested `task/` module owns:
  - `TaskStore`
  - `TaskEntry`
  - `TaskStatus`
  - `TaskSnapshot`
  - Task tool implementations for `TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList`
  - Task reminder hooks and state
  - snapshot/restore/rewind/compaction façade
- `crates/tools` owns only low-level generic tools and helper state such as filesystem tools, `ScopedFs`, `Tracker`, Bash, Web tools, etc.

## Requirements

- Remove Task domain ownership from `tools`.
  - Prefer deleting `crates/tools/src/task.rs` if no non-Pod production caller needs it.
  - Remove `TaskStore`, `TaskEntry`, `TaskStatus`, `TaskSnapshot`, and `task_tools` re-exports from `tools` unless a carefully justified temporary compatibility path is needed.
- Move Task tool implementations into the Pod built-in Task feature module.
  - Tool names, schemas, descriptions, outputs, and behavior must remain unchanged.
  - Preserve once-materialized tool identity and descriptor-approved contribution checks.
- Keep `tools::core_builtin_tools(...)` for non-Task low-level tools.
- Audit `tools::builtin_tools(...)`.
  - If only tests/legacy paths use it, either remove it or change those tests/callers to use `core_builtin_tools` plus the Task feature registry path.
  - Do not keep a production `tools::builtin_tools(..., task_store, ...)` API that implies Task belongs to `tools`.
- Update Pod Task feature code to refer to local Task types, not `tools::TaskStore` / `tools::TaskEntry` / `tools::TaskStatus`.
- Update tests.
  - Move TaskStore/tool tests from `tools` to `pod` where appropriate.
  - Update `tools` integration tests so they no longer assume Task tools are registered by `tools::builtin_tools`.
  - TUI tests currently use `tools` as a dev-dependency for Task compatibility checks. Either replace those checks with local JSON fixtures / shared literal schemas, or document and keep a test-only compatibility helper only if absolutely necessary.
- Preserve current observable behavior:
  - Task tool API and output text;
  - TaskStore replay/snapshot/restore semantics;
  - Task reminder behavior;
  - TUI parsing compatibility;
  - normal ToolRegistry / PreToolCall permission path.

## Non-goals

- Creating a new `builtin-features` crate.
- Extracting `pod::feature` / `pod::hook` into a separate feature-api crate.
- External plugin loading, WASM, package approval, or sandbox authority work.
- Moving Memory, WorkItem, Web, filesystem, or Pod-management tools.
- TUI UI changes.
- Changing Task tool names/schemas/behavior.

## Acceptance criteria

- Task domain state and tool implementation no longer live in `crates/tools` as production API.
- The built-in Task feature module is the owner of TaskStore, Task types, Task tools, and Task reminders.
- `tools` remains a lower-level tool helper crate without Task feature state.
- Production code does not call `tools::task_tools` or `tools::TaskStore`.
- Tests are relocated/updated and continue to verify Task behavior and TUI compatibility.
- Workspace validation passes.
