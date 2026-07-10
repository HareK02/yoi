<!-- event: create author: "yoi ticket" at: 2026-07-10T15:53:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T15:54:24Z -->

## Decision

Worker / Session / workdir retention discussion from Objective 00001KWW44EXK:

- Worker is the archival unit; Session/transcript belongs under Worker archive.
- Worker retention must support pinned to protect selected history from cleanup/prune.
- Workdir files are cache when clean and reconstructable from repository + selector/resolved commit.
- Dirty workdir is an active/recovery state; dirty orphan should require recovery Worker or explicit discard, not automatic prune.
- Backend should keep canonical Worker/Workdir/link records in SQLite, while Runtime owns materialized files, raw paths, process/cwd binding, and cleanup execution.
- Runtime direct/unmanaged workdirs must be distinguishable from Backend-managed workdirs.


---

<!-- event: intake_summary author: hare at: 2026-07-10T16:06:07Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T16:06:07Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
