<!-- event: create author: "yoi ticket" at: 2026-07-17T23:04:40Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T23:15:15Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T23:15:15Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-17T23:27:20Z -->

## Implementation report

Implemented `SessionReferenceView` common substrate.

Changes:
- Added `crates/worker/src/session_reference.rs` with immutable view construction over a session history slice.
- The view produces User/Assistant overview items and a searchable evidence index over user/assistant/system/tool references.
- Added text search with `kind` filter and tool input/output/both filtering via `tool_part`.
- Added bounded read by id or `entry_range` with truncation metadata.
- Added source-ref resolution by evidence id for future extract staging anchors.
- Rewired compaction `search_session_log` to search through `SessionReferenceView` while leaving `read_session_items` behavior unchanged for now.

Validation:
- `cargo fmt --check`
- `cargo test -p worker session_reference`
- `cargo test -p worker compact::worker`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T23:27:20Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-17T23:27:20Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-17T23:27:20Z status: closed -->

## 完了

Implemented `SessionReferenceView` common substrate.

The Worker crate now has an immutable session reference view that can build User/Assistant overview items, index user/assistant/system/tool references, search with kind and tool input/output filters, perform bounded reads with truncation metadata, and resolve source refs for future extract staging anchors. Compaction search now uses this common view; read-session behavior remains unchanged for compatibility.

Validation passed:
- `cargo fmt --check`
- `cargo test -p worker session_reference`
- `cargo test -p worker compact::worker`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`


---
