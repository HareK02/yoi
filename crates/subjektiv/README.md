# subjektiv

Shared Host domain implementation for per-Subject Memory, change history, candidate dispositions, derived surfaces, and Session activity Journals.
`feature-storage` manages physical SQLite storage; the calling Host handles authentication and execution authority.
Journal storage does not generate prose, launch Jobs, or fetch Session history.
Hosts retain body ownership across inactivity, reserve actual execution quiescence, and reconcile final committed history before releasing a Journal writer lifetime. [`journal::binding`] fences new mutations inside storage transactions; historical attribution and exact receipt recovery cannot renew a released lifetime. [`journal::startup`] selects bounded reference context independently of source retention and long-term Memory.

## Main entry points

- [`SubjektivStore`]: scope-bound records and transactions
- [`MemoryRecord`], [`SurfaceSnapshot`]: change history and derived context
- [`api`]: shared Host operations and bounded responses
- [`job`]: consolidation selection rules and result validation
- [`journal`]: individually committed activity operations, source coverage, retry receipts, and bounded reading

## Explicit Subject deletion

`begin_subject_deletion(subject_id, operation_id)` commits a permanent content-free
fence in this store **before** the Host stops execution or erases external content.
The operation ID is unique within this storage scope. Exact retries return current
saved state; a different key for that Subject or reuse for another Subject conflicts.
Pending fences survive restart and a failed purge. Normal reads, writes, saved-receipt
replay, cached Host/Journal contexts and authority lookup cannot use a fenced Subject.
Subject discovery hides fenced records before pagination. SQL INSERT/UPDATE guards
cover direct and indirect owners, checking both OLD and NEW ownership on UPDATE.
Deleting a grant never restores non-Subject fallback: content-free Worker identity
fences remain, and the Host must consult them before fallback authorization.

The Host uses `subject_deletion` / `pending_subject_deletions` for recovery and
`deletion_subject_workers` / `deletion_subject_sessions` / `deletion_subject_jobs`
only as cleanup inventories, not authority or model-visible content. It must fence
creation reservations, quiesce Workers and in-flight delivery, purge Job inputs/results
and live Job execution copies, and clear resident/derived caches. Source Worker/Session
originals and pre-existing archives have independent retention lifecycles: the Workspace
Host archives source originals rather than erasing them. Those archives may contain
previously injected Memory; Workdirs, Tickets, exports and external provider copies are
also outside Subject content erasure. These boundaries must be explained to the user.
An unfinished, previously authorized Worker retention operation is snapshotted separately
from prior successful removals. Its exact original request is reconciled under the permanent
fence; its disposition, fingerprint and canonical result are never rewritten. Body originals
follow that earlier independent request. For a Subject Job, any archive/diagnostic copies
produced by the unfinished operation require a distinct Runtime physical-erasure acknowledgement
before deletion can complete. Lost responses or failed receipt commits retain the same request
and incomplete state across restart. Successes that already existed at deletion start remain
under their independent archive lifecycle; a later success is not inserted into that snapshot.
The domain rechecks after out-of-DB capture callbacks and before returning Job read
responses; already returned DTOs cannot be revoked by SQLite. Host lifecycle locking
must cover delivery/reinjection of previously fetched content as well as finish.

Only after external cleanup succeeds may the Host call
`finish_subject_deletion(subject_id, operation_id)`. One transaction deletes Subject,
Memory/history, staging/evidence, Surface snapshots/runs, permission sources/receipts,
child grants, writer/session bindings, Journal content/generations/deliveries/receipts,
links, and the Subject's `legacy_record_archive` rows. A purge error rolls back all
those deletions without undoing the separately committed fence. Ordinary history
retention and explicit Journal Session-erasure rules are unchanged for other Subjects.
Retained records contain only deletion operation/time/completion and execution/session
identities needed to prevent reuse; existing content-free Session-erasure markers and
workspace-only migration metadata remain. Other Subjects and storage scopes are untouched.

The deletion migration adds fences without erasing or reactivating existing data. Old `retired`
JSON/SQL records deserialize as `SubjectState::LegacyRetired` and remain readable
until explicitly deleted. There is no current retirement API. Normal Worker deletion
still does not implicitly delete Subject memory.

Completion means logical erasure from domain and Host-managed storage, **not** secure
physical overwriting: SQLite free pages, WAL files, filesystem snapshots, backups and
external copies can retain bytes under their own retention/erasure policies.
