---
id: 20260529-083829-tui-multi-pod-polling
slug: tui-multi-pod-polling
title: Poll Pod list updates in tui --multi
status: open
kind: feature
priority: P2
labels: [tui, pod, multi-pod]
created_at: 2026-05-29T08:38:29Z
updated_at: 2026-05-29T08:38:29Z
assignee: null
legacy_ticket: null
---

## Background

`tui --multi` currently loads the Pod list once and refreshes only on explicit `r` or after a direct send. The dashboard is meant to show multiple live/stopped Pods, but statuses can become stale while the user is watching the screen: running Pods may finish, paused Pods may resume elsewhere, newly-created/restored Pods may appear, and stopped/unreachable entries may need to move sections.

Add lightweight polling so the multi-Pod dashboard remains current without requiring manual refresh.

## Requirements

- Add periodic Pod-list refresh while `tui --multi` is open.
  - Poll only in the multi-Pod dashboard, not while a nested single-Pod view is active.
  - Reuse the existing `MultiPodApp::reload` / Pod list discovery path unless a smaller refactor is clearly needed.
  - Keep manual `r` refresh behavior.
- Preserve local UI state across automatic refreshes.
  - Keep the selected Pod stable by Pod name when possible.
  - Preserve per-Pod composer contents.
  - Preserve scroll/selection in a predictable way when the selected Pod disappears or changes section.
  - Do not clear notices unless the existing notice semantics already do so.
- Poll without blocking terminal input.
  - Replace the current blocking `crossterm::event::read()` loop with a bounded poll/tick pattern or equivalent async-friendly structure.
  - Terminal resize, paste, key handling, and direct send behavior must continue to work.
- Avoid excessive filesystem/socket work.
  - Use a modest interval, e.g. 1-2 seconds, and do not start overlapping reloads.
  - If a reload fails, surface a non-fatal notice and keep the previous list visible.
- Do not introduce hidden Pod protocol behavior.
  - Polling should be a TUI dashboard concern; it should not require new Pod methods/events unless existing discovery APIs are insufficient.

## Acceptance criteria

- `tui --multi` automatically updates Pod list/status after live Pods appear, stop, finish, pause, or become unreachable, without pressing `r`.
- Manual `r` refresh still works and shares the same state-preserving reload behavior.
- Selection is retained by Pod name across reload when possible.
- Composer text for each Pod survives automatic polling.
- Automatic reload failure does not exit the dashboard and produces a visible diagnostic.
- Tests cover state preservation across automatic reload, selection fallback when a selected Pod disappears, and non-overlapping/error handling for poll-triggered refresh.
- `cargo fmt --check`
- Focused TUI tests/checks pass, including multi-Pod tests.
