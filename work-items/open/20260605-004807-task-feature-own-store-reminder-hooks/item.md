---
id: 20260605-004807-task-feature-own-store-reminder-hooks
slug: task-feature-own-store-reminder-hooks
title: Task: move TaskStore and reminders into Task feature
status: open
kind: task
priority: P1
labels: [tasks, hooks, feature-registry, history]
created_at: 2026-06-05T00:48:07Z
updated_at: 2026-06-05T02:00:46Z
assignee: null
legacy_ticket: null
---

## Issue

`TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` have been extracted into a built-in internal feature module, but Task state is still Pod-owned. `Pod` owns `TaskStore`, `PodInterceptor` owns `TaskReminderState`, and `PodInterceptor::pending_history_appends()` contains Task-specific reminder logic.

For the feature/module boundary to be meaningful, Task-specific state and behavior should live in the Task feature module. Pod should provide generic host surfaces: tool registration, hook dispatch, and durable `SystemItem` append. Pod should not know about TaskStore or Task reminder rules.

## Current findings

- `crates/pod/src/feature/builtin/task.rs` currently constructs the Task tool feature with a host-provided `tools::TaskStore`.
- `crates/pod/src/pod.rs` still stores `task_store: tools::TaskStore` and `task_reminder_state: Arc<TaskReminderState>`.
- `crates/pod/src/ipc/interceptor.rs` still stores `TaskStore` and `TaskReminderState`, detects task-management tool calls, and emits `SystemItem::TaskReminder` from `pending_history_appends()`.
- `crates/pod/src/pod.rs` also uses TaskStore snapshots for session restore/compaction/reminder context. Those uses must be audited before removing Pod ownership.
- TUI-facing Task visibility is intentionally out of scope here; if TUI needs a compatibility/status surface, create a follow-up instead of keeping Task ownership in Pod.

## Direction

Move TaskStore and Task reminder state into the Task feature module after `hook-context-system-item-sink` provides a host-mediated SystemItem append handle for Hook contexts.

Target shape:

- `TaskFeature` owns:
  - `tools::TaskStore`
  - Task reminder/cooldown state
  - Task tool contribution construction
  - hooks that track Task tool usage and decide when to emit reminders
- Pod owns:
  - feature registry
  - Hook dispatch
  - ToolRegistry integration
  - durable SystemItem append/commit authority
  - generic history/session machinery
- Pod does not own or special-case TaskStore / TaskReminderState.

## Requirements

- Depend on or first implement `hook-context-system-item-sink`.
- Move TaskStore construction/ownership from `Pod` into the built-in Task feature module.
  - Preserve session restore behavior by giving Task feature whatever history snapshot or restore input it needs, rather than keeping TaskStore in Pod.
  - Preserve one session-lifetime TaskStore shared by all Task tools and reminder hooks.
- Move TaskReminderState and reminder decision logic out of `PodInterceptor` and into Task feature-owned hooks.
  - A tool hook records Task tool usage.
  - A `PreLlmRequest` hook evaluates inactivity/cooldown and appends `SystemItem::TaskReminder` through the host-provided SystemItem append handle.
- Remove Task-specific checks from `PodInterceptor` such as direct `is_task_management_tool` handling for reminder state.
- Preserve current observable behavior:
  - Task tool names/schemas/descriptions/outputs;
  - TaskStore snapshot/restore semantics;
  - task reminder threshold/cooldown/body/source behavior;
  - model-visible history path via `LogEntry::SystemItem` / `Event::SystemItem`;
  - normal ToolRegistry / PreToolCall permission path.
- Audit compaction/session snapshot code that currently reads `self.task_store`.
  - Either route this through a Task feature status/snapshot service, or leave a documented temporary compatibility façade with no Task ownership in Pod.
  - Do not silently drop TaskStore snapshot/resume behavior.
- Keep external-plugin authority model out of this ticket except insofar as the Hook SystemItem append handle is used by a trusted built-in module.

## Non-goals

- TUI UI changes.
- External plugin loading or package approval.
- Generic event channels or dialogs.
- Changing Task tool behavior or adding/removing Task tools.
- Reworking Memory/WorkItem/Pod-management modules.

## Acceptance criteria

- Task feature module owns TaskStore and Task reminder state.
- Pod and PodInterceptor no longer contain Task-specific store/reminder state or task-tool special-casing.
- Task reminder emission is implemented as Task feature Hook logic using host-mediated SystemItem append, not raw `Item` injection.
- TaskStore snapshot/restore/compaction behavior is preserved or explicitly routed through a new feature-owned status/snapshot surface.
- Existing Task tool and Task reminder tests are moved/updated and pass.
- Workspace validation passes.
