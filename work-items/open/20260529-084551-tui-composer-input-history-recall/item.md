---
id: 20260529-084551-tui-composer-input-history-recall
slug: tui-composer-input-history-recall
title: Recall composer input history from cursor boundary
status: open
kind: feature
priority: P2
labels: [tui, composer, input-history]
created_at: 2026-05-29T08:45:51Z
updated_at: 2026-05-29T08:45:51Z
assignee: null
legacy_ticket: null
---

## Background

The TUI currently has manual rewind (`Ctrl+R` / `:rewind`) and queued-input restoration (`Alt+Q`), but it does not have a non-destructive composer input history like shell/readline history. Users should be able to recall previously submitted inputs into the composer without rewinding the Pod/session history.

Add local TUI composer history based on sent inputs. This is an editing convenience only: recalling history must not mutate Pod history, send protocol messages, or create hidden context.

## Requirements

- Record submitted composer inputs in a local input history.
  - Record only non-blank user submissions that are accepted for sending or queued for later sending.
  - Preserve typed segments (`Segment` atoms such as file refs, knowledge refs, workflow invocations, paste/file chips) rather than flattening to plain text.
  - Avoid consecutive duplicate entries when the same input is submitted repeatedly.
  - Keep history bounded to a modest fixed size.
- Add composer recall navigation.
  - When the composer cursor is at the start of the buffer, `Up` recalls the previous input history entry instead of moving within the composer.
  - When browsing history, `Up` continues to older entries and `Down` moves back toward newer entries.
  - When the browse cursor returns past the newest entry, restore the draft that was present before history browsing began.
  - If the cursor is not at the start of the composer, preserve the existing multiline cursor-up behavior.
  - If the cursor is at the end of the composer while browsing history, `Down` should move toward newer history; outside history browsing it should preserve the existing multiline cursor-down behavior.
- Preserve drafts safely.
  - Starting history browsing with a non-empty draft must save that draft and restore it when the user navigates back past the newest history entry or cancels/edits out of history browsing.
  - Editing a recalled entry should leave history-browse mode and edit the recalled content as a normal draft.
  - Submitting a recalled entry should submit normally and append/update history using the same rules.
- Keep the feature local to the TUI client.
  - Do not add Pod protocol methods/events.
  - Do not persist input history across TUI restarts in this ticket unless it falls out naturally from existing snapshot/history data. Session-log user messages remain the authoritative durable record.
  - Multi-Pod dashboard/direct-send behavior can share the same history if simple, but per-Pod history is acceptable if it fits the existing app structure better; document the chosen behavior in tests or code comments.
- Maintain existing controls.
  - `Ctrl+R` / `:rewind` remains destructive rewind picker behavior.
  - `Alt+Q` remains queued-input restoration.
  - Page/turn scroll keys continue to operate on conversation history.

## Acceptance criteria

- After sending several messages, pressing `Up` at the start of an empty composer recalls the most recent submitted input, with typed segments preserved.
- Repeated `Up`/`Down` walks older/newer history entries and restores the pre-browse draft after the newest entry.
- `Up` inside a multiline composer when the cursor is not at the start still moves the cursor within the composer.
- Editing a recalled entry exits history-browse mode and does not mutate stored history until submission.
- Consecutive duplicate submissions do not create duplicate adjacent history entries.
- History recall does not call Pod methods and does not alter visible transcript/history until the recalled input is submitted.
- Tests cover empty composer recall, non-empty draft restoration, multiline cursor-boundary behavior, typed segment preservation, duplicate suppression, and submission from recalled history.
- `cargo fmt --check`
- Focused TUI tests/checks pass.
