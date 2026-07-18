<!-- event: create author: "yoi ticket" at: 2026-07-17T23:54:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: implementation_report author: hare at: 2026-07-18T00:02:34Z -->

## Implementation report

Migrated compaction session exploration to `SessionReferenceView`.

Changes:
- Added `ReadDetail` to `SessionReferenceView` so reads can preserve compact/full semantics.
- Updated `read_session_items` to use `SessionReferenceView::read` with `ReadSelector::EntryRange`.
- Kept compact mode omitting tool arguments / tool result content and full mode bounded.
- Removed old compact-specific session search/format helpers.
- `search_session_log` already used `SessionReferenceView::search` and remains on that path.

Validation:
- `cargo fmt --check`
- `cargo test -p worker compact::worker`
- `cargo test -p worker session_reference`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T00:02:34Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-18T00:02:34Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T00:02:34Z status: closed -->

## 完了

Compaction session exploration now uses `SessionReferenceView` for both search and read paths.

`read_session_items` delegates to `SessionReferenceView::read`, preserving compact/full behavior through a new read detail mode. Old compact-specific search/format helpers were removed.

Validation passed:
- `cargo fmt --check`
- `cargo test -p worker compact::worker`
- `cargo test -p worker session_reference`
- `cargo test -p worker`
- `cargo test -p memory`
- `nix build .#yoi`


---
