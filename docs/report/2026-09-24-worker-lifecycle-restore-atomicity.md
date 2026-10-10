# Worker lifecycle, restore atomicity, and observation freshness

Editorial note (2026-10-10): obsolete state terminology is summarized by purpose below; cited IDs, commits and validation results still describe the original investigation, not new executions.

## Scope

This report records the state-model and crash-boundary problems found while investigating why one restored Worker appeared as `idle` in the Web client and `unknown` in the Runtime CLI. The display mismatch exposed a broader problem: lifecycle, controller state, observation freshness, restart intent, and recovery evidence are represented by overlapping fields without one enforced authority.

The investigation covered:

- `protocol::WorkerStateSnapshot` and `worker::WorkerSharedState`;
- Runtime `WorkerRecord`, explicit and automatic restore, stop, and persistence;
- filesystem `worker.json` and `execution.json` records;
- Runtime subscription projection and Workspace Server registry storage;
- REST `WorkerSummary` projection and Web/CLI consumption;
- the run lifecycle contract in `docs/design/worker-session-state.md`.

No `Restoring` lifecycle state is required. The settled Worker lifecycle should remain `Stopped | Active`; operation progress and an indeterminate external outcome must be modeled separately.

## State authorities in the current implementation

`WorkerSharedState` owns the live controller state. Its `WorkerStateSnapshot` contains foreground command admission state such as idle, running, paused, and maintenance. Runtime receives copies of that snapshot from the execution backend. A copied snapshot is not durable execution identity and is not proof that a controller is still reachable.

Runtime catalog `WorkerStatus` is a different projection. Although it has `Idle`, `Running`, `Paused`, and `Stopped` variants, normal Worker state events update `worker_state` without synchronizing `status`. Restore commit writes `Idle` regardless of the returned controller snapshot. Consequently, `status == worker_state.catalog_status()` is not an invariant, and catalog `Running`/`Paused` cannot be treated as foreground authority.

`SubscriptionWorkerAvailability` is a third axis. It says whether Workspace Server currently has a Runtime observation. It does not say whether an execution is active. `Observed + Stopped` is valid: a connected Runtime is currently reporting a stopped Worker. `Unavailable` means the retained payload is not current.

These meanings need distinct types instead of more combinations of `status`, `worker_state`, `execution_handle`, `execution_metadata_available`, and `restore_intent`.

## Restore is not atomic today

The intended settled transition is:

```text
Stopped -- successful Restore commit --> Active
```

Preflight rejection must leave `Stopped` unchanged. If live preparation began but cleanup is proven complete, the old `Stopped` aggregate must also remain unchanged. Once `Active` is committed, Restore succeeded; a later failure is a new execution or recovery failure and must not retroactively turn the Restore into a failed `Active -> Stopped` transition.

The current implementation does not provide this aggregate atomicity.

### Backend side effects precede Runtime commit

The execution backend can start the controller, register observation, and insert its execution map entry before `Runtime::commit_restored_worker_execution` commits the Runtime catalog record. The per-Worker operation lock serializes lifecycle mutations but does not hide GET or subscription reads. There is therefore a pre-commit window in which the committed catalog remains `Stopped` while a live candidate controller exists. If cleanup cannot be proven, this condition outlives the Restore call.

`Stopped + live execution` is not a valid settled domain state, but it is a physical and potentially observable state in the current implementation. Adding a public `Restoring` lifecycle variant would only expose the implementation leak; it would not make the external side effect atomic.

### Worker persistence is two independent commits

`FsRuntimeStore::write_worker_record` writes `worker.json` and then, when available, writes `execution.json`. Here, each file replacement is atomic, but the aggregate is not.

A concrete counterexample is:

1. restored controller creation succeeds;
2. the new `worker.json` with active/idle identity is written;
3. the new `execution.json` write fails;
4. controller cleanup succeeds;
5. Runtime reports `RolledBack`.

The persisted aggregate was not rolled back: its identity file has already changed while its execution file is old. This outcome must not be classified as a proven rollback. Identity, restore specification, execution binding, and recovery evidence need one atomic aggregate commit.

A manifest pointing to separate immutable aggregate snapshots is not preferable here. The store is single-writer, Worker records are bounded, and the existing same-directory temporary-file replacement already supports one aggregate `worker.json`. A manifest would add a second commit pointer and orphan cleanup without resolving Backend reconciliation.

Even a single-file replacement has a commit-ambiguity boundary. `atomic_write_json` renames the temporary file and then syncs the parent directory. If rename succeeds but directory sync fails, the new record is already visible while crash durability is unknown. Persistence errors therefore need a phase-aware classification: failure before rename is definitely not committed; failure after rename is `CommitOutcomeUnknown` and must never be reported as a proven rollback.

### Indeterminate outcomes are disguised as `Idle`

Backend timeout, unknown backend outcome, and failed cleanup produce `ReconciliationRequired`, but Runtime has no durable recovery variant. `retain_restore_reconciliation_pending` substitutes `status = Idle`, unavailable execution metadata, and explicit restore intent. Tests consequently assert `Idle` for an indeterminate outcome, making a normal active Worker and an unresolved Restore indistinguishable at the lifecycle field.

`ReconciliationRequired` is not a normal foreground state, but it is a real durable recovery condition. It must not be encoded as `Idle` or `Stopped`.

## Reconciliation metadata does not converge across restart

When `PersistedWorkerExecutionState::Unavailable` is written, `write_worker_record` writes only `worker.json`; it does not remove, tombstone, or replace an existing `execution.json`.

The resulting restart behavior depends on the stale file:

- an explicit Restore can lose the request needed for retry after restart and be rejected as metadata unavailable;
- an automatic Restore can rediscover stale automatic metadata and attempt another Restore after an outcome that was previously unknown;
- a stale execution file can be combined with a new identity written before a failed second file write.

Existing tests exercise same-process retry but do not prove restart convergence. Recovery state, stable restore specification, and any candidate execution evidence need to be stored explicitly and atomically. An unavailable marker must replace old authority rather than merely decline to write a new file.

## Stop has a separate crash window

The normal stop path performs backend shutdown before committing the stopped Worker record. It then updates and publishes in-memory state before durable Worker persistence, and it attempts a Runtime snapshot write before the Worker record even though the Runtime snapshot does not contain Worker lifecycle state.

If backend shutdown succeeds and a later persistence step fails:

- the execution is stopped;
- memory and subscribers may have observed `Stopped`;
- disk can still describe an active Worker with automatic restart intent.

A Runtime restart can therefore resurrect an execution that was successfully stopped.

Simply persisting `Stopped` before backend shutdown reverses the inconsistency: a crash can then leave a live execution recorded as stopped. Crossing a filesystem and an external process/provider boundary requires an operation protocol, not only write reordering. A durable operation identifier or journal entry must be recorded before the side effect; startup must reconcile that same operation rather than blindly starting a new one.

The operation journal is not a `Stopping` or `Restoring` Worker lifecycle state. It is recovery evidence surrounding a settled lifecycle value.

## Observation projection defect

Workspace Server retains the last Runtime Worker payload when the Runtime observation becomes unavailable and changes only `availability` to `Unavailable`.

`worker_summary_from_projection` correctly writes the REST string state as `"unavailable"`, but it unconditionally copies the retained `worker_state`. The Web client prefers that snapshot and renders stale `idle`; the Runtime CLI, which has no current snapshot, renders `unknown`.

A stale last observation must not occupy a current-state field. REST should expose typed observation availability. When availability is unavailable, current `worker_state` must be absent. If the last observation is useful for diagnostics, it must be carried in an explicitly named `last_observed_worker_state` field.

The subscription layer also synthesizes `Unavailable + Stopped` when no observation exists. Consumers must not interpret that placeholder `Stopped` as an observed fact.

## Target model

The model needs three orthogonal concepts.

### Settled lifecycle

```rust
enum SettledWorkerLifecycle {
    Stopped,
    Active,
}
```

This is durable activation policy and determines whether startup should recover an execution. It does not contain a controller snapshot.

### Current observation

```rust
enum CurrentWorkerState {
    Stopped,
    Active(WorkerStateSnapshot),
}

enum Observation<T> {
    Observed(T),
    Unavailable {
        last_observed: Option<T>,
    },
}
```

Only `Observed` data is current command/foreground state. A Worker configured as active can temporarily have no current observation during Runtime startup or disconnect.

### Operation and recovery condition

```rust
enum WorkerExecutionRecord {
    Settled {
        lifecycle: SettledWorkerLifecycle,
        restore_spec: RestoreSpec,
    },
    ReconciliationRequired {
        last_settled: SettledWorkerLifecycle,
        restore_spec: RestoreSpec,
        operation: PendingOperation,
        evidence: RecoveryEvidence,
    },
}
```

The exact storage type can differ, but the type must prevent a normal active record and unresolved recovery evidence from sharing the same `Idle` representation. Restore and stop operations need stable operation IDs so retry means continuing or querying the same operation, not creating a duplicate side effect.

A successful Restore must have one linearization point: the atomic persisted aggregate commit. Candidate execution events must not be published as current Worker state before that point. A proven rollback retains the old aggregate bytes. An unknown commit or cleanup result retains durable reconciliation evidence.

## Design document drift

`docs/design/worker-session-state.md` says Runtime durably reserves a monotonically increasing run generation before spawn or restore and materializes `runs/<generation>/`.

The current backend uses a UUIDv7 run directory. The previous `last_run_generation` authority is explicitly removed during migration. The document therefore cannot be used as evidence for current crash recovery semantics. The implementation and document must be reconciled as part of this work; the obsolete monotonic-generation claim should not remain normative.

## Required regression boundaries

Implementation must cover at least:

- unavailable observations never exposing stale `worker_state` as current;
- `Observed + Stopped` remaining distinguishable from unavailable;
- preflight Restore rejection preserving the exact old aggregate;
- controller startup followed by proven cleanup preserving the exact old aggregate;
- failure between identity and execution persistence not being reported as rollback;
- unknown backend outcome surviving restart without duplicate Restore;
- explicit reconciliation retaining enough restore specification to retry after restart;
- automatic reconciliation not becoming a fresh automatic Restore after restart;
- commit failure plus cleanup failure retaining durable evidence;
- backend stop success plus persistence failure not causing automatic resurrection;
- retry of failed stop cleanup using the same operation evidence;
- reconnect replacing unavailable/last-observed data with a current observation.

Fault injection must target each persistence boundary. Same-process assertions alone are insufficient; uncertain Restore and Stop cases require reopening the filesystem store and exercising startup recovery.

## Implementation order

1. Stop publishing stale unavailable snapshots as current REST `worker_state` and add projection regression coverage.
2. Replace split identity/execution authority with an atomic aggregate record or immutable aggregate snapshot commit while preserving validated migration input.
3. Introduce explicit durable reconciliation evidence and stable operation identity for Restore and Stop.
4. Quarantine candidate execution observation until aggregate commit and make retry/restart converge on the recorded operation.
5. Remove catalog foreground-state duplication and update clients to use current observed snapshots.
6. Align `docs/design/worker-session-state.md` with the implemented run identity and recovery protocol.
