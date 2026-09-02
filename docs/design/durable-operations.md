# Durable operation design

Durable operation records make retries converge on the same authorized intent
and preserve the facts needed to explain an externally visible result. They are
not execution traces and must not mirror every Rust function or implementation
step as persisted state.

This document defines the cross-domain rules for operation identity, state,
checkpoints, child operations, failure evidence, and terminal disposition.
Domain code may use different records where its atomicity boundary differs, but
it should classify the operation before choosing a schema.

## Core rule

Persist authority and non-reconstructable facts, not control flow.

A value belongs in durable operation state only when at least one of the
following is true:

- it identifies the caller's stable intent and detects conflicting reuse;
- it freezes authority or configuration that an exact retry must continue to
  use;
- it binds a preallocated or created resource to the operation;
- it records an externally visible side effect that cannot be safely derived or
  repeated;
- it records the final domain result or disposition;
- it provides bounded evidence needed for retry, reconciliation, or audit.

A local step does not become durable merely because it occurs before or after
another function call. If current authority can be reread or the step can be
repeated safely, derive or repeat it instead of adding a stage.

## Resilience boundary

These rules cover normal product retry and recovery boundaries: duplicate
requests, returned errors, timeouts, known partial-completion outcomes, process
restart from the last committed facts, and retries of provider operations with
an explicit idempotency or observation contract.

They do not require the system to survive an unexpected stop at every point
while Rust code is executing. A panic, abort, process kill, machine loss, or
power failure may occur between an external side effect and its next durable
checkpoint. Yoi does not attempt to close every such instruction-level crash
window by writing a stage before and after every `await`, function, or provider
call.

Consequently:

- do not claim general exactly-once execution;
- do not introduce write-ahead stages solely to model arbitrary Rust
  control-flow interruption;
- rely on SQLite transaction atomicity for work inside one database transaction;
- prefer provider idempotency, compare-and-swap, stable resource identity, and
  authoritative observation for external effects;
- when an uncovered crash window cannot be reconciled automatically, retain the
  last committed facts and surface an `unknown` or attention-required
  disposition rather than guessing that the side effect did or did not happen.

A domain may require a stronger crash-consistency contract for a specific
destructive or security-sensitive effect. That requirement must be explicit and
must define the provider protocol, checkpoint ordering, replay behavior, and
reconciliation evidence. It is not implied by calling a record a durable
operation.

## Classify the record before adding state

Not every record containing an `operation_id` is a state machine. Use one of the
following shapes.

### Atomic idempotency ledger

Use an idempotency ledger when all authoritative mutations and result recording
commit in one database transaction.

The record normally contains:

- Workspace and operation identity;
- a fingerprint of stable caller intent;
- the created resource or result identity;
- the committed revision and timestamp where relevant.

It does not need `pending`, `executing`, or intermediate stages. An exact retry
returns the recorded result. Reusing the same operation identity with a
different fingerprint fails.

Repository secret mutation results, Workspace resource creation results, and
transactionally appended domain events are examples of this shape.

### Reservation

Use a reservation when an identity or exclusive right must exist before a later
binding can complete.

Persist factual transitions such as:

- the reserved resource identity;
- the immutable request fingerprint and authority snapshot;
- the concrete resource or assignment bound to the reservation;
- reservation expiry or release evidence when the contract requires it.

Do not model internal dispatch, validation, construction, or callback steps as
reservation states. A nullable result binding or a small `reserved | created`
state can be sufficient when those values correspond to real authority facts.

### Durable side-effect operation

Use a durable side-effect operation when work crosses a database/provider
boundary and a retry needs durable intent or result evidence.

The default lifecycle is deliberately small:

```text
pending -> completed
pending -> failed
failed  -> pending     # only when the domain explicitly permits retry
```

Existing code may use `succeeded` for the successful terminal value; new naming
should prefer `completed`. Do not rewrite applied migrations or historical audit
text only to normalize that word.

The operation should contain:

- stable operation identity and request fingerprint;
- immutable resolved authority needed by an exact retry;
- preallocated resource identity where it prevents duplicate creation;
- only the necessary irreversible checkpoints;
- bounded failure evidence;
- the final result and domain disposition.

`pending` means that the intent remains open and current authority must be
reread before progress. It does not identify which Rust function should execute
next. `failed` records the latest terminal attempt outcome; retryability is an
explicit domain rule, not something inferred from the word. `completed` means
the operation's required result and evidence are durably committed.

### Parent workflow

A parent workflow coordinates domain operations but does not duplicate their
lifecycle.

Persist:

- the parent intent and fencing authority;
- stable child operation identities;
- the final workflow result or disposition;
- bounded attention or decision evidence.

Read child state from the child authority. Do not copy child states, provider
stages, Worker status, attachment status, or Workdir status into a second parent
state machine. A parent cleanup workflow will often need only
`pending | completed`; child failure remains on the child operation and appears
in the parent as current attention metadata.

Creating or binding a child must itself be idempotent. Prefer a deterministic
child operation identity or persist the child reference atomically with the
parent decision so a retry cannot create siblings for one intent.

## Checkpoint rules

A checkpoint records a fact that changes retry semantics. It is not a progress
notification.

Add a checkpoint only when all of the following hold:

1. A side effect may already have occurred outside the current transaction.
2. Current authority cannot derive the fact reliably enough for safe retry, or
   repeating the effect is not safe under the provider contract.
3. The retry algorithm changes after the fact is committed.
4. Tests can exercise behavior before and after the checkpoint.

Prefer factual fields over stage names:

- `provider_deleted_at` is evidence that provider deletion succeeded;
- `child_operation_id` binds delegated work;
- `result_revision` identifies the committed result;
- `target_ref_after` records verified merge evidence.

Avoid fields such as `validating`, `closing_session`, `detaching`,
`deleting_registry`, or `finalizing`. Those names describe code location, not
durable authority. If those steps are safe to rerun or their result can be read
from Worker, attachment, Workdir, repository, or provider authority, they are
not checkpoints.

A checkpoint must never claim more than the authority that produced it. For
example, sending a provider request is not proof that provider deletion
completed, and receiving a Worker notification is not proof that a Ticket or
cleanup workflow completed.

## State, failure, blockers, and disposition are separate

Do not overload one enum with unrelated dimensions.

- **Operation state** says whether the intent is open, completed, or has a
  recorded failed attempt.
- **Failure evidence** records a bounded category, timestamp, and safe
  diagnostic detail for the latest failure.
- **Blockers and eligibility** are normally derived by rereading current
  authority. Persist them only as audit or attention evidence, not as a
  substitute for live validation.
- **Disposition** records what the domain decided to retain, delete, release,
  tombstone, abandon, or leave unknown.
- **Attention metadata** explains why automated progress currently cannot
  continue and what authority must change.

Values such as `blocked`, `executing`, `stale`, `dirty`, `retained`, and
`deleted` therefore do not all belong in one operation-state enum. Some are
derived conditions, some describe transient execution, and some are domain
results.

Before every retry or side effect, reread live authority and revalidate its
fence. A previously recorded blocker does not prove that the operation remains
blocked, and a previously unblocked operation does not retain permission after
assignment, ownership, revision, or attachment authority changes.

## Identity and fingerprinting

Every externally retryable operation has a stable identity in its owning
Workspace or authority scope. The operation fingerprint represents stable caller
intent, not generated results or mutable observations.

Include inputs whose change would mean a different requested operation. Exclude:

- generated resource IDs when the Server allocates and persists them as the
  result;
- timestamps assigned by the Server;
- retry counters and diagnostics;
- current provider observations that are expected to change;
- secret bytes and credential material.

Resolved authority snapshots may be stored separately from the caller
fingerprint. An exact retry uses the persisted snapshot where replay convergence
requires it; a new operation resolves current authority. Unknown, foreign, or
conflicting operation identity fails closed.

## Transactions and external providers

Keep database work in one transaction whenever the owning authority and result
live in the same database. Do not create a durable operation merely to split a
transaction that can remain atomic.

When an external provider is involved:

1. reserve stable intent and identity if retry needs them;
2. invoke the provider with the strongest available idempotency, expected-old
   revision, or stable resource key;
3. verify the provider result through authoritative response or observation;
4. commit only the checkpoint or result evidence that changes retry behavior;
5. on retry, reread both the operation and current domain/provider authority
   before acting.

Compensation is a domain operation, not an invisible `finally` block. If
compensation has its own external side effects or retry lifecycle, give it a
stable child operation identity rather than expanding the parent into a list of
cleanup stages.

## Diagnostics and audit

Persist bounded error categories and identifiers needed to investigate or retry.
Do not persist credentials, provider handles, raw command output, raw prompts,
full session transcripts, or host paths in ordinary operation diagnostics.

Attempt counts, last-attempt timestamps, and safe provider categories may help
operations, but they are telemetry and evidence rather than lifecycle authority.
Logs may describe detailed execution stages; the durable record should remain
centered on intent, checkpoints, result, and disposition.

## Applying this rule

For a new or materially changed operation:

1. identify the owning authority and transaction boundary;
2. classify it as an atomic ledger, reservation, durable side-effect operation,
   or parent workflow;
3. define stable identity, fingerprint, and exact-retry behavior;
4. list external side effects and decide which are idempotent or authoritatively
   observable;
5. add only checkpoints that change retry behavior;
6. keep child operation state in the child authority;
7. separate failure, blocker, attention, and disposition from lifecycle state;
8. state the unsupported crash windows honestly;
9. test fingerprint conflict, exact retry, authority revalidation, checkpoint
   replay, and result/disposition projection as applicable.

Existing operation schemas need not be rewritten solely for vocabulary
consistency. When an operation is changed for functional reasons, use this
classification to remove derived or control-flow stages rather than adding
another special-case lifecycle.
