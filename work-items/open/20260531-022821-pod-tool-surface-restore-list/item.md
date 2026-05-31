---
id: 20260531-022821-pod-tool-surface-restore-list
slug: pod-tool-surface-restore-list
title: Pod tools: unify pod listing and rename restore operation
status: open
kind: task
priority: P2
labels: [pod, tools, orchestration]
created_at: 2026-05-31T02:28:21Z
updated_at: 2026-05-31T02:59:12Z
assignee: null
legacy_ticket: null
---

## Background

The current LLM-callable Pod tool surface has overlapping concepts:

- `ListPods` is registry-oriented and lists Pods spawned by the current Pod.
- `ListVisiblePods` is state/visibility-oriented and lists visible live/stored Pods.
- `InspectPod` provides a named detail lookup.
- `AttachOrRestorePod` has an ambiguous name: the "attach" branch mostly detects an existing live socket and returns connection information; it does not clearly mean that the target becomes a comm-registered child for `SendToPod` / `ReadPodOutput`.

Recent multi-agent workflow incidents also showed that stopped/restorable child Pods and missing/unreachable children are not clearly represented. For LLM tool use, fewer, more precise operations are preferable.

User decision:

- Integrate `ListVisiblePods` semantics into `ListPods`.
- Remove `InspectPod` from the LLM tool surface.
- Rename `AttachOrRestorePod` to `RestorePod`.

## Requirements

- Replace the old `ListPods` result semantics with the state/visibility-aware listing currently represented by `ListVisiblePods`.
  - The tool should list Pods visible to the current Pod, not host-wide Pods.
  - Visible set should include self and spawned children according to current visibility rules.
  - Preserve enough fields to distinguish live/reachable, stored/restorable, missing/corrupt/unrestorable, and comm/registry state where available.
  - If preserving exact `PodList`/`PodListEntry` structures is easier, use them; do not keep two nearly identical listing tools.
- Remove `ListVisiblePods` as a separate LLM-callable tool.
  - Update tool registration, tool descriptions, tests, prompts/docs if they mention it.
  - If protocol enum compatibility must remain for non-LLM/TUI callers, keep internal compatibility only and do not expose it as an LLM tool; otherwise remove it cleanly.
- Remove `InspectPod` from the LLM-callable tool surface.
  - Update tool registration, tool descriptions, schema, tests, prompts/docs.
  - Prefer `ListPods` for discovery/detail and `RestorePod` for action.
  - If protocol enum compatibility must remain, keep it internal/non-advertised only; otherwise remove it cleanly.
- Rename `AttachOrRestorePod` to `RestorePod` in the LLM-callable surface.
  - The operation should restore a visible stopped/restorable Pod, or report that a visible Pod is already live.
  - Avoid using “attach” terminology in tool name/description unless a real comm-registry materialization semantics is implemented.
  - Update tool schema, descriptions, registration, tests, and prompts/docs.
  - Decide whether to keep a backwards-compatible protocol alias internally; avoid exposing both names to the LLM.
- Preserve scope, visibility, and safety boundaries.
  - Do not make host-wide Pod enumeration available.
  - Do not allow restoring Pods outside the caller’s visibility set.
  - Do not broaden delegated scope handling in this ticket.
- Keep behavior for `SpawnPod`, `SendToPod`, `ReadPodOutput`, and `StopPod` coherent with the new listing/restore tools.
  - If full stopped/restorable child semantics require a larger registry change, document it as follow-up rather than half-implementing it.

## Non-goals

- Full redesign of stopped/restorable child lifecycle and delegated scope reclaim semantics.
- Changing Pod registry durable authority or parent restore pruning behavior.
- Host-wide Pod management tools.
- TUI dashboard redesign.

## Acceptance criteria

- The LLM-visible tool list exposes `ListPods` and `RestorePod`, and no longer exposes `ListVisiblePods`, `InspectPod`, or `AttachOrRestorePod`.
- `ListPods` returns visibility/state-aware data sufficient for the LLM to decide whether a Pod is live, stopped/restorable, or unavailable.
- `RestorePod` replaces the old `AttachOrRestorePod` name in tool descriptions and tests.
- Existing tests are updated and new/renamed tests cover:
  - `ListPods` includes visible stored/live entries formerly covered by `ListVisiblePods`;
  - `RestorePod` rejects invisible/missing Pods;
  - `RestorePod` reports already-live visible Pods without spawning another process;
  - removed tools are not registered in the LLM tool registry.
- Any intentional protocol compatibility aliases are documented in code comments and are not advertised to the LLM.
- `cargo fmt --check`, focused Pod/tools tests, `cargo check` for affected crates, `./tickets.sh doctor`, and `git diff --check` pass.
