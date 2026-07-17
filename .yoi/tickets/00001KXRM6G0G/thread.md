<!-- event: create author: "yoi ticket" at: 2026-07-17T18:07:40Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-17T18:10:00Z -->

## Decision

Created to replace the previous batch staging direction. This Ticket owns the new flat staging record schema: one candidate per staging record, with kinds `preference`, `working_assumption`, `constraint`, `decision`, `open_question`, and `lesson`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T19:48:08Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T19:48:08Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-17T19:56:08Z -->

## Implementation report

Implemented flat candidate staging record schema.

Changes:
- Replaced extract payload with `candidates[]` based on narrow candidate kinds: `preference`, `working_assumption`, `constraint`, `decision`, `open_question`, `lesson`.
- Added flat `StagingRecord` schema with `schema_version`, `id`, `extract_run_id`, record-level `source`, `kind`, `claim`, `why_useful`, optional `staleness`, bounded `evidence[]`, and `source_refs[]`.
- Changed staging writer to write one JSON file per candidate. Empty payload writes no files.
- Updated transitional `write_extracted` tool and extract prompt to candidate extraction instead of activity-log extraction.
- Updated consolidation staging/input paths and tests to read/render flat records.
- Updated worker extract path to handle multiple staged candidate records per extract run.

Validation:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-17T19:56:08Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-17T19:56:08Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-17T19:56:08Z status: closed -->

## 完了

Implemented flat candidate staging record schema.

The memory extract staging path now treats staging as flat candidate records: one extract run can produce zero or more staging files, and each staging file is one consolidation decision unit. The transitional `write_extracted` path now accepts `candidates[]`, and the host expands each candidate into a flat `StagingRecord` with schema version, ids, source, kind, claim, usefulness, optional staleness, bounded evidence, and source refs.

Validation passed:
- `cargo fmt --check`
- `cargo test -p memory`
- `cargo test -p worker`
- `nix build .#yoi`


---
