---
id: 20260529-084551-tui-composer-input-history-recall-review
slug: tui-composer-input-history-recall
title: Review for composer input history recall
status: reviewed
kind: review
created_at: 2026-05-29T08:45:51Z
updated_at: 2026-05-29T09:18:00Z
reviewer: insomnia-system
---

## Review summary

Reviewed implementation branch `work/tui-composer-input-history-recall` in worktree `/home/hare/Projects/insomnia/.worktree/tui-composer-input-history-recall`.

The implementation adds local-only typed composer input history, bounded to 100 entries, records non-blank sent/queued inputs, suppresses adjacent duplicates, supports cursor-boundary `Up` recall and browsing with `Up`/`Down`, restores pre-browse drafts, and exits browse mode on edit. It does not add Pod protocol changes or mutate transcript/history until a recalled input is submitted normally.

One blocking issue was found and fixed: pressing `Up` at the buffer start with an empty input history cleared a non-empty draft because browsing captured the draft with `submit_segments()` before confirming that history existed. The amendment avoids mutating the composer when no recall entry exists and adds a regression test.

## Validation

Reviewer ran:

- `cargo fmt --check`
- `cargo test -p tui input_history`
- `cargo test -p tui recalled`
- `cargo test -p tui multiline`
- `cargo test -p tui empty_composer`
- `cargo test -p tui up_at_start_with_empty_history_preserves_draft_without_browsing -- --nocapture`
- `git diff --check develop...HEAD`

All passed. The only compiler warning observed was the pre-existing `llm-worker` `end_scope` dead-code warning.

## Judgment

Approved after amendment.
