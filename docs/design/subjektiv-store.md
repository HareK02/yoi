# subjektiv subject Memory store

T-667 introduces a trusted Server-side `subjektiv` repository in
`yoi_workspace_server::subjektiv`. It is a domain store, not a Worker tool or a
cutover of the existing Workspace Memory authority.

## Ownership and scope

- The Server-managed `FeatureStorage` owns the SQLite path, connection, common
  pragmas, migration lifecycle, backup/restore, shutdown, and Workspace deletion.
- subjektiv owns its migration contents, tables, issued subject/Memory/snapshot
  identifiers, validation, queries, and domain transactions.
- Each repository is bound to the Workspace of its `FeatureDatabase`. A
  `store_scope` row verifies that binding on every open. Subject IDs select data
  inside that boundary; knowing an ID grants no authorization.
- Authorization must happen before calling the repository. Neither a subject ID
  nor Memory text can confer Workspace permissions or assignments.
- No SQLite path, connection, or SQL surface is sent to a Worker or Runtime.
  Async callers must run these synchronous repository calls on a blocking
  executor.
- The store contains no `current_worker_id` or equivalent current link. Yoi's
  keyed singleton remains the sole authority for the current Worker. T-668
  connects Workers using the opaque `subjektiv:<subject-id>` key and records
  immutable historical Worker/Session attribution as provenance; that history is
  never consulted as a current Worker link.

Registration is deliberately separate from open. Trusted Server construction
registers `REGISTRATION` once per `FeatureStorage` manager, then reuses its
`RegisteredFeature` for each Workspace. Merely adding this module does not open
or create a subjektiv database. T-668 opens it lazily for explicit subject
operations or an authenticated subject extraction write; leaving the Feature
disabled has no subject-store side effect.

## Versioned records

All new persisted JSON records use `schema_version: 1`. Subject staging retains
existing extract `StagingRecord` schema version 2 and its six kinds:
`preference`, `working_assumption`, `constraint`, `decision`, `open_question`,
and `lesson`. The host adds only `subject_id` and `created_at`; the model does
not provide subject scope or provenance.

A subject record has a store-issued random ID, an explicit role, lifecycle state,
and `store_revision`. It starts `active` at generation 0. Retirement is terminal
in this baseline and is distinct from stopping a Worker. Retirement prevents new
writes but retains all reads and history.

A Memory revision contains:

- a stable store-issued Memory ID and positive revision;
- subject ID, existing six-way kind, state, claim, optional Markdown body,
  `why_useful`, and optional staleness condition;
- exact source candidate IDs and exact `{memory_id, revision}` derivations;
- a required change reason and host-issued creation/update times.

Revision 1 must start `active`. For every kind, `active` may remain active or
become `resolved`/`retracted`; `resolved` may remain resolved, reopen to active,
or be retracted. `retracted` is terminal. A corrected experience after
retraction receives a separate Memory ID and may cite the old fixed revision.
`active` means eligible for ordinary recall, not proven true and not authorized.

Refinement/correction of the same experience writes the next revision under the
same Memory ID. A distinct experience or lesson gets a new ID. Every write uses
an expected current revision; stale callers receive a typed `RevisionConflict`.
Old rows, evidence edges, and derivations are immutable and retained.

A surface snapshot stores only its body, exact Memory revision references, the
subject's `built_from_store_revision`, and host-issued ID/time. Creation requires
the generation to equal the subject's current store generation and permits at
most one fixed revision of each Memory. The snapshot may deliberately select a
bounded subset; selection, generation, bounds, and injection policy are later
work.

## Relational invariants and retention

The feature database contains `subjects`, `staging_records`,
`staging_resolutions`, `memory_records`, immutable `memory_revisions`, normalized
candidate/derivation edges, and immutable `surface_snapshots` with normalized
references. Composite foreign keys always include subject scope. As the physical
database is already Workspace-scoped, Workspace identity is stored once in
`store_scope` rather than copied into every row.

`memory_records` is the current-revision pointer and current JSON projection;
`memory_revisions` is the immutable historical authority. Candidate JSON is
immutable. A resolution stores both the resolution JSON and the exact staged JSON
bytes that were resolved. Aggregate seal rows prevent evidence, derivations,
resolution targets, or snapshot references from being appended after their
parent JSON is finalized. Foreign keys plus retention triggers prevent dangling
candidate, derivation, resolution-target, and snapshot references. There is no
record deletion API in this baseline.

Search columns and JSON projections are written together by the typed repository.
Raw SQL remains private trusted implementation detail. Existing Workspace Memory
rows are neither imported nor dual-written.

## Transactions

`FeatureDatabase::try_transaction` preserves typed Feature-domain errors while
retaining the same `BEGIN IMMEDIATE`, rollback, lifecycle fencing, and connection
ownership as `transaction`.

`SubjektivStore::apply_candidate` and `apply_candidates` atomically:

1. verify the active subject and every unresolved candidate in repository scope;
2. check the expected Memory revision and every exact candidate/derivation ref;
3. derive the revision's candidate provenance from that same candidate set;
4. insert and seal the immutable Memory revision and evidence edges;
5. update the current Memory projection and increment `store_revision` once;
6. insert and seal each immutable candidate resolution and exact target revision.

Generic candidate resolution cannot emit `applied`, and direct Memory writes
cannot attach candidate provenance; those relationships must go through this
atomic path.

Any error rolls the complete operation back. The transaction is confined to the
subjektiv Feature database; it does not promise atomicity with the Server
control-plane database or another Feature database.

## Operational contract

Reopening the same Server-managed feature database verifies migrations and the
stored Workspace scope before serving reads/writes. `WorkspaceFeatureStorage`
online backup/restore includes the entire subjektiv database, including immutable
history and provenance. Workspace deletion and shutdown use the common lifecycle
fences and close retained repository handles. Disabling subjektiv does not erase
its database.

## Extraction connection (T-668)

`feature.subjektiv` is an explicit Worker profile boundary. It reuses the
committed-run threshold, restricted Internal extraction Worker,
`SessionEntryRef` evidence resolution, explicit finish, generation fence, and
success-only pointer progression from the established Memory lifecycle. Its
pointer domain is separate from legacy Workspace Memory, and a Worker is rejected
at installation when both automatic extraction destinations are enabled. Ordinary
SubWorkers and Reviewer SubWorkers have lifecycle Features disabled by their host;
only evidence actually committed into the parent Session can enter the parent's
capture.

The staging request contains the existing model contract (`kind`, `claim`,
`why_useful`, optional `staleness`, and entry references after host resolution)
plus the committed parent `session_id`. It contains no subject id. The Server
derives subject scope from authenticated Runtime/Worker source proof and the
current keyed-singleton lease, then attaches the session id to every source
reference and records immutable historical `{subject, runtime, worker, session}`
provenance atomically with staging. A Session already attributed to another
subject is rejected. Preference candidates still require exclusively HumanInput
evidence.

Subject retirement does not stop a Worker, and stopping/replacing a Worker does
not retire the subject. A replacement uses the same `subjektiv:<subject-id>` key;
retries therefore recover through Yoi's singleton authority without a second
current-worker mapping in this database.

Candidates are immutable writes that occur before `FinishMemoryExtraction`. If a
run stages one or more candidates and then fails, is cancelled, or loses its
generation fence, those candidates remain recorded and the extraction pointer
does not advance. A retry may therefore produce semantically duplicate candidates;
subsequent consolidation must compare them using the existing candidate
identity/content boundary. The system must not claim that the partial writes were
rolled back or silently delete them. Subject recall, consolidation, and legacy
Memory migration remain separate work.

## Recall and explicit proposal tools (T-669)

An enabled `feature.subjektiv` installs five names that do not overlap the legacy
single-Markdown Memory tools: `SubjektivMemoryQuery`, `SubjektivMemoryRead`,
`SubjektivMemoryListRevisions`, `SubjektivMemoryRemember`, and
`SubjektivMemoryProposeRevision`. Runtime-signed Worker source plus current
`subjektiv:<subject-id>` singleton ownership is re-evaluated for every operation;
none of the model-visible inputs contains a subject, Runtime, Worker, Session,
origin, or raw provenance object.

Query searches only current projections. Omitted `states` means `active`; explicit
empty `states` or `kinds` is invalid. Output defaults to 20 and is capped at 100,
ordered by `updated_at DESC, memory_id ASC`. Its opaque cursor binds the subject,
canonical filters, offset, and subject `store_revision`; any confirmed-Memory
change makes it a typed stale-cursor conflict rather than silently mixing
snapshots. Revision conflicts and stale cursors retain their diagnostic code as a
structured `{status: "error", error: {code, message}}` tool result instead of
requiring models to parse prose. Revision history is ordered by revision
descending. Its first page
fixes the maximum revision, so later revisions neither duplicate nor displace old
page members.

Read accepts a positive exact revision or resolves the current revision once. A
missing historical revision never falls back to current. Markdown pagination is
line-based (default 200, maximum 1000) with an additional 16 KiB UTF-8 response
budget. It reports `body_truncated`, `body_next_offset`, and
`body_next_byte_offset`; an oversized single line resumes from the returned byte
boundary within that same line. Every continuation offset must also supply the exact
revision returned by the first page, so a current-revision change cannot mix body
versions. Provenance uses a separate immutable-revision-bound cursor and returns one
candidate/derivation reference per page. A staged candidate contains at most 10
evidence records and 10 source references, all returned together under a hard 40 KiB
serialized budget for the expanded candidate array; displayed anchor text is capped
at 64 UTF-8 bytes. Every staging path rejects larger anchor sets, so accepted
provenance is never made unreachable. Candidate evidence is the bounded
host-resolved anchor saved in staging; raw Session bodies are not copied
into Memory responses. Resolved, retracted, and historical revisions remain
addressable by ID.

Remember and ProposeRevision only stage candidates. Entry references are resolved
against the host's committed Session capture, and preference candidates continue
to require exclusively `HumanInput` evidence. When no entry is supplied, a
non-preference request returns `pending_commit`; after the run commits, the host
reconstructs pending receipts from the durable tool-call/tool-result history and
uses the committed tool-call entry itself as model-origin evidence. Receipt lookup
reports `pending_commit`, `staged`, or `missing`; the post-commit hook retries
immediately, the pre-request hook replays durable pending receipts after Worker
restore, and the fail-closed pre-rewrite hook prevents compaction from dropping an
unstaged receipt. Hook failure and process restart therefore retain an idempotent
retry path, while an operation that never commits is not staged. Receipt identity is derived from Session and tool-call identity,
and exact backend retries return the first candidate rather than creating another
one. Neither path changes `memory_records`, `memory_revisions`, `store_revision`,
or a surface snapshot.

Revision proposals add optional `revision_proposal` metadata to the existing v2
`SubjectStagingRecord` envelope; automatic extraction remains proposal-free and
its model schema is unchanged. The metadata is serialized atomically with the
candidate and therefore remains present in candidate reads and immutable
resolution copies:

```json
{
  "intent": "revise",
  "memory_id": "memory-…",
  "expected_revision": 3,
  "change_reason": "The committed evidence corrects the prior condition"
}
```

This is the T-670 handoff fixture. Consolidation must re-read this typed metadata,
re-check subject ownership, `expected_revision`, and the requested state
transition in the same transaction that applies the candidate, and leave a stale
proposal unresolved/conflicted. It must not parse target information from the
claim, retarget to the latest revision, or revive a retracted Memory. Valid
transitions at staging are revise while active/resolved, active→resolved,
active/resolved→retracted, and resolved→active. Retraction remains terminal.
