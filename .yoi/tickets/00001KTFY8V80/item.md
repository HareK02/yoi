---
title: "Preserve active workflows across compaction"
state: 'inprogress'
created_at: "2026-06-07T02:23:28Z"
updated_at: '2026-06-14T16:17:48Z'
queued_by: 'workspace-panel'
queued_at: '2026-06-14T15:23:07Z'
---

## Background

Long-running orchestration work often spans compaction. Workflow text can be strongly available in the turn where the workflow is invoked, but after compaction the durable state may only retain a loose summary. This can cause the agent to forget that it is still operating under workflow constraints such as worktree/reviewer/merge/commit handling from `multi-agent-workflow` and `worktree-workflow`.

The problem is not merely summarization quality: active workflow invocation state and workflow obligations should be represented durably enough to survive compaction.

## Goal

Preserve active Workflow invocations and their operational obligations across compaction so long-running workflow-governed tasks continue to follow the invoked workflow after context pruning/compaction.

## Requirements

- Represent user-invoked or otherwise active workflows as durable, typed history/state rather than only transient context text.
- Preserve at least:
  - workflow slug;
  - invocation source/time;
  - intended task/scope when available;
  - whether the workflow remains active or has completed;
  - concise current obligations/checkpoints relevant to the active workflow.
- Compaction must carry active workflow state forward explicitly.
- After compaction, context construction must be able to rehydrate active workflow guidance from durable state.
- The implementation must not inject workflow instructions into model context based only on turn-local/transient information that is absent from history/state.
- Workflow guidance after compaction should distinguish:
  - workflow availability/advertisement;
  - workflow currently active for this task.
- Decide and document whether rehydration uses the latest workflow body by slug or an invocation-time snapshot; make the choice explicit.
- Active workflow obligations should be cleared or marked completed when the workflow-governed task finishes, so stale workflow constraints do not leak into unrelated work.
- Include coverage for at least a long-running worktree/multi-agent style flow where compaction occurs between review delegation and merge/close handling.

## Non-goals

- Rewriting workflow content itself.
- Solving all memory summarization quality issues.
- Automatically invoking workflows without user or orchestration intent.
- Making workflows immutable unless the snapshot/latest-body decision chooses that explicitly.

## Acceptance criteria

- A workflow-governed task can cross compaction without losing the fact that `/worktree-workflow` or `/multi-agent-workflow` is active.
- The post-compaction context clearly distinguishes active workflow obligations from merely available resident workflows.
- Workflow instructions that affect model behavior are traceable to durable history/state.
- Stale active workflow state is cleared at task completion or explicit cancellation.
