---
id: 20260529-083829-tui-multi-pod-polling-review
slug: tui-multi-pod-polling
title: Review for tui --multi Pod list polling
status: reviewed
kind: review
created_at: 2026-05-29T08:38:29Z
updated_at: 2026-05-29T09:12:00Z
reviewer: insomnia-system
---

## Review summary

Reviewed implementation branch `work/tui-multi-pod-polling` in worktree `/home/hare/Projects/insomnia/.worktree/tui-multi-pod-polling`.

The implementation adds a modest dashboard polling interval, avoids overlapping reloads with a single pending reload handle, shares the state-preserving reload application path with manual refresh, and aborts the polling task while a nested single-Pod view is active. Reload failures keep the previous list visible and surface a notice.

The implementation is scoped to the TUI dashboard and does not introduce Pod protocol changes. Selection, composer state, and notices are preserved across successful reloads as required.

## Validation

Reviewer ran:

- `cargo fmt --check`
- `cargo test -p tui multi_ -- --nocapture`
- `git diff --check develop...HEAD`

All passed. The only compiler warning observed was the pre-existing `llm-worker` `end_scope` dead-code warning.

## Judgment

Approved.
