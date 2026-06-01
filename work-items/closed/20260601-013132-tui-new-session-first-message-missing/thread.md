<!-- event: create author: tickets.sh at: 2026-06-01T01:31:32Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-01T02:00:33Z -->

## Plan

## Investigation summary

Read-only investigation found that the most likely cause is live-event ordering between `Event::UserMessage` and the first `Event::SegmentRotated` in a fresh session.

Observed flow:

1. TUI submit path calls `App::submit_input()` / `method_for_run()` and sends `Method::Run { input }`.
2. `crates/pod/src/controller.rs` handles `Method::Run` and currently broadcasts `Event::UserMessage { segments }` before calling `pod.run(input)`.
3. TUI receives `Event::UserMessage` and appends a `TurnHeader` / `UserMessage` block.
4. `Pod::run()` calls `prepare_for_run()` / `ensure_segment_head()`.
5. In a fresh session, `entries_written == 0`, so `LogEntry::SegmentStart` is committed only at this point.
6. `crates/pod/src/segment_log_sink.rs` converts `SegmentStart` into `Event::SegmentRotated`.
7. TUI handles `SegmentRotated` by clearing blocks through `reset_for_rotation()` and replaying `SegmentStart.history`.
8. The current `UserInput` has not been committed yet, so `SegmentStart.history` does not contain the first user message.
9. Later `LogEntry::UserInput` is committed, but the live path does not emit `Event::UserMessage` from that commit, so the cleared user block is not restored in the live TUI view.

Likely affected files / functions:

- `crates/tui/src/app.rs`
  - `App::submit_input()`
  - `method_for_run()`
  - `App::handle_pod_event(Event::UserMessage)`
  - `App::handle_pod_event(Event::SegmentRotated)`
  - `reset_for_rotation()`
- `crates/pod/src/controller.rs`
  - `Method::Run` branch
- `crates/pod/src/pod.rs`
  - `Pod::run()`
  - `prepare_for_run()`
  - `ensure_segment_head()`
- `crates/pod/src/segment_log_sink.rs`
  - session-log-derived live event conversion

## Implementation intent

Move the live `Event::UserMessage` authority away from the controller's pre-`pod.run()` optimistic broadcast and toward the persisted `LogEntry::UserInput` commit path.

Preferred shape:

- Remove the controller-side `Event::UserMessage` broadcast that happens before `pod.run(input)`.
- Emit `Event::UserMessage` when `LogEntry::UserInput` is committed.
- Prefer doing this in the session-log-derived event lane, such as `SegmentLogSink`, so `SegmentStart`, `UserInput`, and other replayable entries share a single ordering source.

This should make a fresh session produce `SegmentRotated` first and then `UserMessage` for the committed input, preserving the first user message in the TUI after rotation.

## Requirements / invariants for implementation

- Do not fix this by adding a TUI-only fake/pending user block after rotation.
- Do not reintroduce local optimistic user blocks in `App::method_for_run()`.
- The displayed message should correspond to `LogEntry::UserInput` / persisted history.
- Existing session attach/restore must continue to replay `UserInput` from `Snapshot.entries`.
- 2nd and later sends must continue to display normally.
- Running-state queued input should display when the queued run is actually accepted/committed, not merely when typed.
- Composer input history should remain TUI-local and unaffected.

## Suggested validation

- Add or update a Pod/session-log event ordering test for a fresh session's first `Method::Run`, asserting the live event stream makes the committed user message visible after the initial segment rotation.
- Add or update TUI app/view-model tests if available:
  - Current problematic ordering `UserMessage` then `SegmentRotated` clears the block.
  - Corrected ordering `SegmentRotated` then `UserMessage` leaves the block visible.
- Confirm snapshot restore still creates user blocks from `LogEntry::UserInput`.
- Run focused tests for `pod` / `tui` crates as appropriate, plus formatting/checks that are reasonably scoped to the touched code.


---

<!-- event: review author: hare at: 2026-06-01T02:23:10Z status: approve -->

## Review: approve

## External review summary

Status: approved

Reviewer inspected the ticket, investigation artifact, and implementation commit `501dcc9 fix: show initial TUI user message` in branch `tui-new-session-first-message-missing`.

Findings:

- The implementation matches the ticket intent and investigation result.
- The controller-side pre-run optimistic `Event::UserMessage` broadcast is removed from the authoritative path.
- Live `Event::UserMessage` emission is tied to persisted `LogEntry::UserInput` / session-log-derived ordering.
- Fresh sessions should now see initial `SegmentRotated` before the committed user message event, preserving the first message in the TUI view.
- The fix does not use a TUI-only fake/pending message.
- No unnecessary protocol/schema expansion was found.
- Existing snapshot restore, later sends, queued input, and composer input history did not show obvious regressions from the diff.

Validation reviewed:

- `cargo fmt`
- `cargo test -p pod`
- `cargo test -p tui`

Parent decision needed: none.


---

<!-- event: close author: hare at: 2026-06-01T02:23:11Z status: closed -->

## Closed

Merged branch tui-new-session-first-message-missing via merge commit ff27917 after external review approval. Validation on develop passed: cargo fmt --check, cargo test -p pod, cargo test -p tui, cargo check -p insomnia, ./tickets.sh doctor.


---
