# subjektiv subject Memory store

The trusted Server-side `subjektiv` repository in
`yoi_workspace_server::subjektiv` is the persistence authority for subject-scoped
Memory. It is a domain store, not a Worker tool. Legacy single-document Workspace
Memory remains a separate, deprecated control-plane authority until an operator
performs the explicit reset defined by the
[subjektiv product cutover runbook](../development/subjektiv-product-cutover.md);
legacy rows are never imported, dual-written, or silently reinterpreted as subject
Memory.

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

## Immutable changes and schema contracts

All new persisted JSON records use `schema_version: 1`. Subject staging retains
existing extract `StagingRecord` schema version 2 and its six kinds:
`preference`, `working_assumption`, `constraint`, `decision`, `open_question`,
and `lesson`. The host adds only `subject_id` and `created_at`; the model does
not provide subject scope or provenance.

A subject record has a store-issued random ID, an explicit role, lifecycle state,
and lifecycle timestamps. It starts `active` with no confirmed Memory. Retirement is terminal
in this baseline and is distinct from stopping a Worker. Retirement prevents new
writes but retains all reads and history.

User-managed `behavior_md` is separate from generated Memory context. Updating it compares the exact previously read text through `expected_behavior_md`, not a behavior counter. Conflicts require rereading the actual current text; changing behavior neither changes confirmed Memory nor rewrites Worker history.

An immutable Memory change contains:

- a stable store-issued Memory ID, opaque `change_id: String`, and optional `previous_change_id`;
- subject ID, existing six-way kind, state, claim, optional Markdown body,
  `why_useful`, and optional staleness condition;
- exact source candidate IDs and exact `{memory_id, change_id}` derivations;
- a required change reason and host-issued creation/update times.

The first change must start `active`. For every kind, `active` may remain active or
become `resolved`/`retracted`; `resolved` may remain resolved, reopen to active,
or be retracted. `retracted` is terminal. A corrected experience after
retraction receives a separate Memory ID and may cite the old fixed change.
`active` means eligible for ordinary recall, not proven true and not authorized.

Refinement/correction of the same experience appends a new immutable change under the
same Memory ID. A distinct experience or lesson gets a new ID. Every write uses
`expected_change_id` equal to the current change ID; stale callers receive a typed `MemoryChangeConflict`.
Old rows, evidence edges, and derivations are immutable and retained.

`memory_fingerprint` is SHA-256 over the sorted, length-framed `(memory_id, current_change_id)` set, including all Memory states. Each committed content/state/provenance change gets a fresh change ID, preventing ABA reuse; staging alone does not change this fingerprint.

A surface snapshot stores only its body, exact Memory change references, the
subject's `built_from_memory_fingerprint`, and host-issued ID/time. Creation requires
the captured fingerprint to equal the current `memory_fingerprint` and permits at
most one fixed change of each Memory. The snapshot may deliberately select a
bounded subset; selection, generation, bounds, and injection policy are later
work.

## Relational invariants and retention

The feature database contains `subjects`, `staging_records`,
`staging_resolutions`, `memory_records`, immutable `memory_changes`, normalized
candidate/derivation edges, and immutable `surface_snapshots` with normalized
references. Composite foreign keys always include subject scope. As the physical
database is already Workspace-scoped, Workspace identity is stored once in
`store_scope` rather than copied into every row.

`memory_records` is the `current_change_id` pointer and current JSON projection;
`memory_changes` is the immutable historical authority. Candidate JSON is
immutable. A resolution stores both the resolution JSON and the exact staged JSON
bytes that were resolved. Aggregate seal rows prevent evidence, derivations,
resolution targets, or snapshot references from being appended after their
parent JSON is finalized. Foreign keys plus retention triggers prevent dangling
candidate, derivation, resolution-target, and snapshot references. Ordinary
subject operations expose no record deletion API; the legacy reset contract does
not touch this Feature database.

Search columns and JSON projections are written together by the typed repository.
Raw SQL remains private trusted implementation detail. Existing Workspace Memory
rows are neither imported nor dual-written.

## Transactions

`FeatureDatabase::try_transaction` preserves typed Feature-domain errors while
retaining the same `BEGIN IMMEDIATE`, rollback, lifecycle fencing, and connection
ownership as `transaction`.

`SubjektivStore::apply_candidate` and `apply_candidates` atomically:

1. verify the active subject and every unresolved candidate in repository scope;
2. check the expected Memory change ID and every exact candidate/derivation ref;
3. derive the change's candidate provenance from that same candidate set;
4. insert and seal the immutable Memory change and evidence edges;
5. update the current Memory projection and recompute the fingerprint of current Memory identities;
6. insert and seal each immutable candidate resolution and exact target change.

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
`SubjektivMemoryListChanges`, `SubjektivMemoryRemember`, and
`SubjektivMemoryProposeChange`. Runtime-signed Worker source plus current
`subjektiv:<subject-id>` singleton ownership is re-evaluated for every operation;
none of the model-visible inputs contains a subject, Runtime, Worker, Session,
origin, or raw provenance object.

Query searches only current projections. Omitted `states` means `active`; explicit
empty `states` or `kinds` is invalid. Output defaults to 20 and is capped at 100,
ordered by `updated_at DESC, memory_id ASC`. Its opaque cursor binds the subject,
canonical filters, offset, and subject `memory_fingerprint`; any confirmed-Memory
change makes it a typed stale-cursor conflict rather than silently mixing
snapshots. Change-ID conflicts and stale cursors retain their diagnostic code as a
structured `{status: "error", error: {code, message}}` tool result instead of
requiring models to parse prose. Change history follows the immutable `previous_change_id` chain, newest first. Its first page fixes `head_change_id`, so later appended changes neither duplicate nor displace old page members. IDs are not counters and are not ordered numerically.

Read accepts an exact nonempty `change_id` or resolves the current change once. A
missing historical change never falls back to current. Markdown pagination is
line-based (default 200, maximum 1000) with a preliminary 16 KiB UTF-8 body
cap and a global 56 KiB budget over the exact pretty-serialized model-visible
response. JSON escaping, provenance, and metadata therefore reduce the current
body page before the Worker content ceiling is reached. It reports
`body_truncated`, `body_next_offset`, and `body_next_byte_offset`; an oversized
or escape-heavy single line resumes from the returned UTF-8 byte boundary within
that same line. Every continuation offset must also supply the exact `change_id`
returned by the first page, so a current change cannot mix body contents.
Provenance uses a separate immutable-change-bound cursor and returns one
candidate/derivation reference per page. New staged candidates contain at most 10
evidence records and 10 source references; read pages project at most two of each,
with displayed anchor text capped at 64 UTF-8 bytes and nested offsets carried by
the ordinary evidence cursor. Compatible candidates persisted before the admission
cap remain readable across as many evidence pages as needed, while every current
staging path rejects new larger anchor sets. Candidate evidence is the bounded
host-resolved anchor saved in staging; raw Session bodies are not copied
into Memory responses. Resolved, retracted, and historical changes remain
addressable by ID.

Remember and ProposeChange only stage candidates. Entry references are resolved
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
one. Neither path changes `memory_records`, `memory_changes`, the confirmed-Memory fingerprint,
or a surface snapshot.


## Historical Session discovery (T-673)

An enabled `feature.subjektiv` also installs exactly three read-only tools:
`SubjektivSessionList`, `SubjektivSessionSearch`, and `SubjektivSessionRead`.
The Server binds subject, Workspace, Runtime, Worker, retention catalogue, and
archive authority from the authenticated caller. Model input contains only
Session/segment/entry selectors, search filters, limits, and opaque cursors;
knowing any identifier does not grant access. Reads use Yoi's retained or
committed-archive Session storage and a shared public projection. They do not
copy Session bodies into the subjektiv database, restore Workers, migrate
Session storage, create candidates or Memories, revise a surface, or promote a
re-read tool result to HumanInput.

The public projection covers committed records in every persisted segment,
including non-active branches and pre-compaction entries, while excluding
system prompts, hidden reasoning, traces/diagnostics, unfinished run tails, and
attachment or pasted-artifact bodies. Results preserve exact Session, segment,
entry, provenance, and lineage identity so inherited entries are not presented
as a current decision. Full entry reads are UTF-8-boundary paged and remain
subject to the same projection as snippets and search.

The subject's first committed primary-Worker Session is attributed independently
of extraction thresholds or candidate creation. Recording is idempotent and is
retried on later committed-run and pre-request lifecycle points after transient
failure; no attribution is attempted before a committed capture exists. This is
not a distributed transaction with Session persistence. In particular, the
system does **not** backfill older unattributed Sessions from their text, display
name, current singleton ownership, Memory references, or other inference.
Historical Sessions without trustworthy Host-recorded attribution remain outside
subject discovery until a separately authorized provenance mechanism exists.

Pagination cursors bind scope, operation, filters, stable ordering, and the
public storage generation and are never authorization credentials. Every page
rechecks current subject and observation authority. Deleted-Worker archives are
eligible only when both the committed Server catalogue and Runtime manifest
agree and the archived observation grant remains valid; incomplete, expired,
corrupt, unavailable, or changed sources are reported explicitly rather than as
an empty successful search.

Change proposals add optional `change_proposal` metadata to the existing v2
`SubjectStagingRecord` envelope; automatic extraction remains proposal-free and
its model schema is unchanged. The metadata is serialized atomically with the
candidate and therefore remains present in candidate reads and immutable
resolution copies:

```json
{
  "intent": "revise",
  "memory_id": "memory-…",
  "expected_change_id": "change-…",
  "change_reason": "The committed evidence corrects the prior condition"
}
```

This is the T-670 handoff fixture. Consolidation must re-read this typed metadata,
re-check subject ownership, `expected_change_id`, and the requested state
transition in the same transaction that applies the candidate, and leave a stale
proposal unresolved/conflicted. It must not parse target information from the
claim, retarget to the latest change, or revive a retracted Memory. Valid
transitions at staging are revise while active/resolved, active→resolved,
active/resolved→retracted, and resolved→active. Retraction remains terminal.

## Candidate consolidation and corrections (T-670)

subjektiv requests a bounded Backend-owned Job with the explicit
`builtin:subjektiv-memory-consolidation` Profile and an immutable Subject/candidate
batch grant. The common runner resolves the recipe; Profile names do not grant
permissions. The Host derives authority from the authenticated current
Job/attempt/Worker binding; model-visible inputs cannot select a Workspace,
subject, Runtime, Worker, or Session. Its tools list/read only pending candidates
in that batch, query/read confirmed changes, and make one candidate decision.
Candidate text is evidence rather than authority. No subject-specific idle Worker
is resubmitted; see [Job execution and cutover](subjektiv-consolidation-jobs.md).

Every decision carries a stable request ID. An exact retry returns the committed
receipt, while reuse with different input is a conflict. Applying a candidate
atomically writes one new confirmed change, its exact candidate and derivation
edges, the immutable resolution and affected change references, and updates
the subject Memory fingerprint. A non-applied decision explicitly records
`discarded`, `invalid`, `duplicate`, or `already_covered`; an empty model response,
transport failure, or aborted consolidation records no disposition and leaves the
candidate pending. A successful applied response is the only evidence that
confirmed Memory changed.

A typed change proposal must be applied to its exact `memory_id` and
`expected_change_id`, preserving its intent and change reason. The transaction
rechecks the current change ID and state transition. It never rebases or retargets
a stale proposal, revives retracted Memory, or extracts a target from claim prose.
Correction/refinement of the same experience appends under the same Memory ID;
a corrected experience after terminal retraction receives a new Memory ID and may
cite the retracted fixed change as a derivation. Surface generation is a
separate post-consolidation lifecycle: its failure cannot roll back a committed
candidate decision.

For the product activation, legacy reset, and operator recovery boundary, follow
the [subjektiv product cutover runbook](../development/subjektiv-product-cutover.md).
