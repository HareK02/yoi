# Guarded Worker Restore and Web observations

T-701 extends the [registry-bound operation reception](workspace-worker-operations.md), not the Worker creation, Resume or Restore state machines.

## Different tokens protect different things

| Operation | Observed identity / expected input | Atomic validation and execution owner | Conflict / uncertain outcome | Shared client behavior / deliberate difference |
| --- | --- | --- | --- | --- |
| Restore | Workspace + Runtime + Worker identity; Runtime-owned `restore_observation_token`; immutable `expected_observation_token` + `request_id` | Existing Runtime per-Worker lifecycle operation lock, durable admission and lifecycle commit. Preparation and execution follow admission, not a frontend GET comparison | Stale new intent is HTTP 409 (`restore_observation_conflict`); operation results remain accepted / rejected / rolled_back / reconciliation_required. A lost reply is unknown, not definite rollback | Both entrances use the same scoped POST/parser/notice classifiers and observation refresh. Never replace a rejected intent's token and automatically retry |
| Stop | Identity and reason, **not** a catalog content CAS | Runtime per-Worker operation lock and existing backend Stop settlement; Workspace session finalization only after accepted | Existing flat accepted / unsupported / rejected result with diagnostics | Same encoded target paths and bounded HTTP-error messages; refresh observations instead of writing a guessed local stopped state. Stop does not gain Restore's observation-token admission guarantee |
| Pause / Resume / Cancel / Compact | Connected execution identity and `WorkerCommandEnvelope.command_id` | Existing bound execution/controller command admission | Command acknowledgement: Accepted / StaleCommandId / Conflict / InvalidState, with authoritative state | Command IDs order commands; they are not catalog observation tokens. Resume is not Restore; connections do not silently move to a replacement execution |
| Delete / cleanup | Runtime + selected target IDs; `expected_plan_digest` of the candidate set, dependencies and blockers | Existing cleanup validation, per-target live/retention revalidation, removal service and catalog commits | Existing cleanup-plan conflicts, per-target status and diagnostics | Preserve plan protection for candidate sets and dependencies. Never substitute a Worker observation token. Page plans invalidate at the beginning of every catalog refresh and are unavailable until fresh catalog and plan observations settle |
| Pin / Unpin | Registry Worker identity and explicit boolean | Registry retention update and ordered projection publication | Existing retention result/errors | Preserve retention semantics and full catalog metadata; refresh shared observations and cleanup plan |
| Submit / Notify | Authenticated source + durable submission / notification request ID | Existing controller/session durable receipt admission | Existing accepted/rejected receipts and unknown delivery diagnostics | These are not Restore requests; Restore never manufactures input, Resume or a new Worker |

There is no universal Worker command token or shared frontend action lock. Workspace reception resolves a stable target and rechecks caller authorization; Runtime/controller/store owners retain operation-specific atomic boundaries.

## Browser request and result

Normal explicit Restore is enabled only for `availability = observed`, catalog `stopped`, and an available backend observation token. Sidebar display state and foreground `worker_state` are not eligibility evidence: sidebar uses `lifecycleState`; the page uses the full catalog's `state`. Neither UI guesses snapshot, singleton, Workdir provider or authorization eligibility.

Both entrances send the generated two-field `WorkerRestoreRequest` to:

```text
POST /api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/restore
```

Each ID is independently encoded. No Ticket-assignment query, Submit, Resume or Worker-create call is used. The bounded parser validates wrapper identity, the four states, diagnostics and optional **Workspace WorkerSummary**. A Stop lifecycle result or launch summary is not a Restore response. Unknown states/fields/invalid identities cannot produce a success notice.

Accepted means the existing Restore operation committed, not that the Worker is necessarily Running or a later run succeeded. A returned summary validates result identity; display is updated from fresh shared observations, not an old POST snapshot. Rejected and rolled_back display diagnostics without success. Reconciliation_required and transport/malformed-response uncertainty retain the original immutable request for an explicit **Retry Restore** action. That action sends the same request; it does not create an operation, exchange its observation token, or automatically loop. A conflict clears this local recovery intent and asks the user to review fresh observations before a new explicit operation. A GET alone does not settle Runtime's pending lifecycle operation.

## Backend admission, preparation and recovery

The Workspace `WorkspaceWorker` reception still resolves target identity and rechecks caller authority. Its Restore coordinator first dispatches the public tuple to Runtime without candidate preparation. A terminal owner returns its historical receipt; an admitted pending owner supplies its stored preparation before renewed singleton/Workdir/External eligibility is evaluated. Historical result summaries are read from the current Runtime and are not written back as catalog observations.

For a new intent, Workspace builds read-only preparation references after the normal eligibility checks. Runtime atomically checks the observation token again under the existing per-Worker lock and persists the owner together with `pending_restore` and its existing lifecycle operation ID. Only this admitted operation can proceed to SSH resource issuance, binding/access preparation and execution. A race between the first observation check and admission therefore cannot issue credentials or launch a new Restore from the old intent.

Preparation completion is attached to that same pending operation, not a replacement operation. Retained repository-access snapshots and exact request identity allow interrupted preparation to resume without exchanging candidate bindings. Runtime retries use the original owner's payload; a freshly generated preparation cannot rewrite the admitted target or expected observation. Stop and startup reconciliation continue to use the existing pending lifecycle ownership. When startup/legacy persistence has a pending lifecycle operation but no public receipt owner, an explicit request with the current observation may bind recovery to that exact operation ID, without applying candidate preparation. Once bound, a different request cannot take over; terminal replay does not resurrect a later stopped Worker.

| Caller | Intent identity retained / validation boundary |
| --- | --- |
| Web sidebar and page | Immutable two-field request retained locally for explicit uncertain-result recovery; no shared in-flight lock |
| Workspace alias, scoped and Worker-control HTTP | Mandatory public request; common target/auth reception and the same Runtime admission |
| Embedded / remote Runtime providers and management HTTP | Typed Runtime request and coordination transport, including original observation and lifecycle operation owner |
| `WorkerRestore` Tool | Observed token in Tool arguments and durable Tool-call-derived request identity |
| Existing TUI Restore/resume path | Selected row's token and exact request retained through the existing flow. Unknown/pending results refresh Session observation and prompt for an explicit same-tuple retry; cancellation does not retry and reports the original tuple. Conflict exits for fresh selection, never exchanges the failed intent. No new TUI Restore entrance |
| Internal Coder, Orchestrator, subject reuse and compensation | Domain intent journal pins the request/token before dispatch and retains it through unknown result and restart. A definitive observation conflict retires only that rejected intent without automatic retry; a later deliberate invocation may pin fresh observation with a fresh request ID. It does not own lifecycle status or bypass Runtime validation |
| Runtime startup / pending reconciliation | Private recovery of the existing persisted Restore owner and operation; not an external unguarded new-intent endpoint |

The Workspace internal intent journal stores client retry identity only. Runtime remains the lifecycle authority; it is not a second Restore state machine, Worker ledger or cross-screen lock.

## Observation and cleanup-plan ordering

`workspaceWorkersStore` retains the existing sidebar stream/projection and also fetches the full Workspace Worker catalog for the page. Full catalog metadata preserves retention, attachments, implementation and targets that are not sidebar entries. Stream updates overlay lifecycle/availability/foreground snapshot/token without interpreting absence as a new page exclusion.

Every stream update or explicit refresh increments a local **observation scheduling version** and publishes `catalogRefreshing` before fetching. This local version fences asynchronous display work; it is **not** Restore admission authority and is never sent to Runtime. A newer event/GET supersedes older GET completion. Old POST summaries and page-loader data cannot overwrite the active catalog. On disposal, requests and subscription callbacks are fenced. Catalog fetch errors have their own stable alert episode rather than clearing transport/Workdir alerts.

The page invalidates cleanup plans immediately when observations refresh. It does not fetch or install plans while `catalogRefreshing` is true. Successful refresh completion triggers plan reads even when the resulting catalog contents did not change. Workspace/lifetime and cleanup epochs reject late previous plan responses. A failed refresh leaves old candidates unavailable instead of treating old page data as fresh.

Both components retain local busy state only. Requests pin their starting Workspace/target, and late replies after Workspace switch/disposal cannot update another Workspace's notices, list or busy state. Sidebar and page may issue competing requests: atomic Runtime admission—not a cross-component client lock—decides their outcomes. Console links and existing sidebar filtering remain unchanged.

## Validation surfaces

- Runtime admission tests prove stale/ABA and serialization, durable same-request replay and pending reconciliation.
- Runtime management, embedded/remote providers, Workspace aliases/scoped/control routes, Tool and existing TUI callers exercise the guarded API rather than a legacy unguarded Restore.
- `tests/runtime-workers.test.ts` and `test/sidebar/worker-actions.test.ts` prove response/request and notice contracts; the Runtime parser tests are included in the normal Web test task.
- Rendered `worker-stop.browser.spec.ts` and `workers-page.browser.spec.ts` cover menu/action eligibility, busy behavior, four outcomes and errors, explicit retry, Workspace/disposal fences, ordinary Console/Pin/Delete, sidebar-origin updates and cleanup epochs.
- `worker-restore-store.browser.spec.ts` tests the real store's full metadata, GET/event ordering, dependent-plan invalidation, no POST recovery loop, catalog-only target preservation and disposal.

Exact executed validation counts and immutable source/review evidence belong to the repository Merge Request and final approved Ticket handoff.
