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
  keyed singleton remains the sole authority for the current Worker. Historical
  Worker/Session attribution is provenance and will be connected separately.

Registration is deliberately separate from open. Trusted Server construction
registers `REGISTRATION` once per `FeatureStorage` manager, then reuses its
`RegisteredFeature` for each Workspace. Merely adding this module does not open
or create a subjektiv database; product enablement and old-Memory cutover are
later work.

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
