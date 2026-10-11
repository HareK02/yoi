# Backend bounded synchronous execution

## Ownership and admission

`ControlPlaneStore::db_executor()` returns a clone of the bounded execution owner.
Use `executor.run(move || synchronous_operation(...)).await` with owned input and
services. The closure owns the **whole** synchronous operation, including connection
acquisition, mutex wait, the existing SQL transaction, commit, and ordered local
publication. Do not split source authorization, receipts, CAS, audit, or saved
results around an await. Do not change connection ownership or introduce a read pool.

Production stores share one DB worker and a queue of 32 jobs. Admission uses a
nonblocking queue send and a separate actual-job budget (worker count plus queue
capacity); rejection means the operation was **not accepted**. HTTP maps capacity
and executor unavailability to 503, not authentication failure or deletion conflict.
Tests can inject a dedicated executor with explicit limits, without process-global
settings or environment variables.

Accepted work is owned by the worker, not the Tokio waiter. Cancelling a waiter:

- does not interrupt, resubmit, or retry SQL;
- does not return its queue/job slot before execution and discarded-result cleanup;
- does not release the request lifecycle lease before actual completion.

Synchronous post-commit continuations inherit the admitted lifecycle source when
submitting another bounded job. This is not implicit inheritance by detached Tokio
tasks. An operation panic has an unknown outcome even if SQL already committed;
return a structured unknown-outcome error and diagnostic, never retry the mutation.
A cancelled fire-and-forget job still emits its diagnostic. `drain()` observes all
jobs accepted before its barrier is first polled and waits for actual cleanup; it
is not a queue close and does not include later admissions. Lifecycle callers must
stop their producers/admission before destroying resources.

## Runtime/network boundaries

A synchronous Runtime workflow must not monopolize the sole DB worker while
waiting for callbacks that themselves need DB authority. The Server's separate
finite Host lane has two workers and 16 queued jobs. It uses the same accepted-job
ownership mechanism, not an unbounded `spawn_blocking` queue.

Pure DB stages use the DB lane. Existing mixed resource-lifecycle workflows can
use the Host lane, with no Runtime I/O or await inside a SQL transaction. Where an
owned workflow needs async resource locks, `run_host_workflow` polls it on a Host
worker using the original Tokio handle; do not build a replacement runtime for
cached services and do not submit to and await the Host lane from inside itself.
Control/session guards needed by an accepted mutation belong to that actual job.

Backend Job cleanup is admitted before its durable cleanup claim. Rejection leaves
Pending/Failed ownership recoverable instead of abandoning a Running claim. Ticket
checker dispatch and timeout processing use bounded admission; overload leaves the
saved reservation/deadline diagnosable. They are not automatically retried after an
unknown DB outcome. Runtime teardown is still its own lifecycle fence, not proof
of model processing.

Remote subscription and Workdir authorization are async callbacks: their complete
live binding check and fresh request signing are offloaded together. A queue error
must not fall back to a configured bearer token. Synchronous CLI/migration callers
and existing synchronous Runtime adapters retain their explicit blocking boundary.

## Caller recipe (including assignment APIs)

```rust
let store = api.store.clone();
let workspace_id = api.workspace_id().to_owned();
let worker = worker.clone();
let record = api.store.db_executor().run(move || {
    store.get_worker_registry(&workspace_id, &worker)
}).await?;
```

This is an illustration, not a new authority API. Assignment delivery scan,
reserve, prepare, acknowledgement and start authorization must keep their existing
whole transaction helpers. Additional assignment diagnostics, answer mutations,
and Ticket queries use the same owned caller boundary; do not add a second ledger
or reconstruct processing evidence from Worker liveness.

## Regression evidence

- `db_executor::tests`: real SQLite IMMEDIATE write contention while a current-thread
  timer/control task progresses; accepted/cancelled queue bounds; rollback; panic
  before/after commit; discarded reply cleanup and retained authority; child jobs.
- `server::db_execution_tests`: HTTP Ticket writes and cold/warm Subjektiv scope
  opening/mutation under a separate SQLite connection's write lock; overload;
  cancelled HTTP jobs and Workspace deletion drain.
- `worker_projection::tests`: real SQL contention, commit/payload publication,
  ordered snapshot registration, and replacement draining a cancelled consumer's
  accepted SQL before applying a new Runtime snapshot.
- Auth/proof and Runtime subscription tests protect availability classification,
  replay rejection, and no bearer fallback.
- `workdir`'s `http-client` tests protect awaited fresh authorization and exact
  request claims before any HTTP send.

These fixtures use temporary authority data and observable synchronization, not
live databases or a fixed sleep to guess completion. This change does not claim a
latency benchmark or a measured production incident.
