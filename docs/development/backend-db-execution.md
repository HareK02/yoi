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
settings or environment variables. Ordinary SQLite fixtures use `open_for_test`,
`in_memory_for_test`, or `from_connection_for_test`: their clones share their own
1-worker/32-queued owner, not the production singleton or another fixture's drain.
Server/Workspace fixture constructors likewise inject a lazy `HostExecutors` owner;
all child Workspaces and weak retention adapters use that same owner. Host and
cleanup still have the production 2-worker/16-queued limits. Dropping the final
sender closes its queue after accepted work finishes; idle fixture threads are
not kept in a process-global fixture cache.

The restore intent journal and Workspace deletion use
`with_immediate_transaction`: read authority/journal state only after acquiring
SQLite write ownership. A DEFERRED read-to-write upgrade can return BUSY without
invoking SQLite's busy handler when another connection is writing. Moving these
whole helpers to an offloaded caller must not create an automatic mutation retry.


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
Subjektiv session Read/Search, subject response projection, general memory staging
and legacy consolidation-scope helpers also use Host ownership: they can
synchronously call Runtime/network services outside their SQL transactions.
Pure DB-resident Subjektiv scope and memory projection helpers stay on the DB lane.
Host admission must not be acquired again from an already-owned Host workflow,
including indirectly through a Runtime's mandatory Backend callback.

Mandatory Session attribution has Subject-only authority and whole attribution
SQL on the independent DB lane. Store-only scope discovery distinguishes Subject,
current Job consolidation, and legacy consolidation authority; attribution rejects
both consolidation roles **before** any Runtime profile/tag lookup. Legacy
consolidation checks for mixed callers remain on Host. Subject-only memory
operations (resident context/summary, proposal validation, receipt lookup, explicit
staging) use the same store-only authority boundary and DB ownership. Runtime
bootstrap/restore can request resident context while its Host parent is waiting.
This is not permission to bypass Workspace deletion admission: only the already
allowed exact callback endpoints may reenter deletion drain.

Backend Job cleanup has a third finite lane (two workers, 16 queued jobs), so a
long Run teardown wait cannot starve ordinary Host control/stop requests. It is
admitted before its durable cleanup claim. Rejection leaves Pending/Failed
ownership recoverable instead of abandoning an Executing claim. Ticket
checker dispatch and timeout processing use bounded admission; overload leaves the
saved reservation/deadline diagnosable. They are not automatically retried after an
unknown DB outcome. Runtime teardown is still its own lifecycle fence, not proof
of model processing.

Remote subscription and Workdir authorization are async callbacks: their complete
live binding check and fresh request signing are offloaded together. Cold protocol
source lookup in `connect_workspace_worker_protocol` is also awaited as an owned
DB stage, rather than invoking its synchronous signing callback on Tokio.
`WorkspaceWorkerRuntime::observation_source` is fallible: live binding/signing
failure and bounded admission errors propagate explicitly, not as a missing
source or a fallback to a configured bearer token. Synchronous CLI/migration
callers and existing synchronous Runtime adapters retain their explicit blocking
boundary.

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

Worker discovery uses the same whole `workers_response` DB stage as Worker list
and resource detail: projection snapshot, Workdir registry/links, and authority
projection never acquire the shared connection on Tokio. Source-proof requirement
and pure query/cursor validation still precede admission; overload remains 503.
Runtime resource list/detail use an accepted Host workflow for synchronous Runtime
summary I/O and await independent DB binding/audit reads. Launch options are also
mixed Runtime/config helpers and use Host, not the sole DB worker. Runtime
registration validates its signing identity in a whole DB stage, scans existing
Workers on Host, then owns SQL key/CAS mutation, local registration publication
and response observation in one accepted workflow. Metadata updates likewise keep
commit and publication together after waiter cancellation. Public resource/detail
helpers admit Host ownership; already-owned workflows call their `_on_host`
variants directly, never await another admission to the same occupied lane.
Connection tests ping on finite Host ownership and then await the whole current
binding fence on DB. A replaced/revoked binding cannot attest the earlier ping.
Do not confuse a Runtime summary callback with a pure stored Worker projection
or run network waits on the sole DB worker.

## Regression evidence

- `db_executor::tests`: real SQLite IMMEDIATE write contention while a current-thread
  timer/control task progresses; accepted/cancelled queue bounds; rollback; panic
  before/after commit; discarded reply cleanup and retained authority; child jobs.
- `server::db_execution_tests`: HTTP Ticket writes and cold/warm Subjektiv scope
  opening/mutation under a separate SQLite connection's write lock; overload;
  cancelled HTTP jobs and Workspace deletion drain; same-budget fixture isolation,
  saturated/cancelled Host jobs versus cleanup and paused Tokio time; restore intent
  contention before the first authority read, with one persisted tuple and no retry.
  The terminal checker fixture keeps its original Tokio driver alive through accepted cleanup,
  then shuts down projection producers and drains actual DB jobs.
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
