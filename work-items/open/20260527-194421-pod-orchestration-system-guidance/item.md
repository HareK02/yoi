---
id: 20260527-194421-pod-orchestration-system-guidance
slug: pod-orchestration-system-guidance
title: Pod orchestration tool availability に応じた system guidance
status: open
kind: feature
priority: P2
labels: [pod, workflow, prompt]
created_at: 2026-05-27T19:44:21Z
updated_at: 2026-06-01T01:24:27Z
assignee: null
legacy_ticket: null
---

## Background

Child Pod completion/status notifications are delivered as non-blocking background signals. Parent Pods that have Pod management tools should treat notifications for Pods they spawned as actionable orchestration state, but should not block the active turn merely to wait for output.

Current guidance is too weak: agents may either ignore routine child-Pod follow-up until the user asks, or waste a turn with `sleep`/polling while waiting for a notification. The desired behavior is notification-driven follow-up at a natural stopping point.

Prompt text belongs under `resources/prompts`; Rust code should only assemble it conditionally.

## Acceptance criteria

- Pod management toolsが有効な Worker の system prompt に orchestration guidance が含まれる。
- Pod management tools が無効な Worker には含まれない。
- prompt 本文が `resources/prompts` にある。
- guidance includes:
  - spawned Pod notifications are background signals the parent should handle at a natural stopping point;
  - the parent does not need to keep a turn open or call tools solely to wait for a notification;
  - do not use `sleep`/polling loops just to wait for Pod output;
  - read child output/diff/test evidence before treating delegated work as complete;
  - do not start scheduler/auto-maintain behavior or bypass user/workflow authorization.
- Prompt assembly tests cover conditional inclusion/exclusion.
- Related focused tests and `cargo fmt --check` pass.
