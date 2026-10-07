# Host-owned standalone Jobs (T-708)

## Implementation design recorded before code changes

Base: `3c424fd17d1a5e8bb5542de2d0ad88f55338e0ea` (current origin/develop; T-704 integrated). T-709's subjektiv consumer is not part of this change.

The reusable `job` crate owns the transport-neutral immutable request (registry Profile selector, input snapshot/reference/revision, caller Job ID, instruction, serialization key and bounded limits), result envelope/digest and validation. Backend retains its existing SQLite state machine, Runtime binding, domain grants, authorization, unknown-outcome and cleanup implementation; its adapter uses the shared request validation and result contract, not a second runner.

Standalone owns a separate adapter, not a WorkspaceClient impersonation. `StandaloneHost` owns a Job service and its connection lifecycle. The concrete local placement is `<standalone state directory>/<WorkerId>/jobs.sqlite3` (using the existing Worker directory layout; alongside, not inside, interactive sessions). The `standalone` crate owns the versioned local schema: durable intents, immutable attempts, execution state, accepted result/digest, cancellation and consumer acknowledgement. A single Host connection is protected by the existing Worker lease. SQLite transactions serialize reserve, dispatch, result acceptance and terminal transitions. No subjektiv tables or fake Workspace/Runtime registry are created.

Local adapter states follow the existing intent/attempt/result distinctions. Exact immutable request replay returns the same Job, never starts another attempt. Explicit retry creates a bounded new attempt only for known failure; unknown outcomes require caller reconciliation and never auto-replay. Unstarted pending attempts remain pending on shutdown/reopen. Interrupted dispatch/running attempts without an accepted result become unknown; accepted results survive restart and remain available until consumer acknowledgement. Cancellation is durable and never auto-resumed. Consumer acknowledgement/delivery failure is separate from model success and never reruns the model.

Execution uses the existing private Internal Worker substrate with a new Host-facing bounded Job entry point. It gets a distinct ephemeral session, explicitly resolved selected Profile/system prompt/model, no ambient parent tools, filesystem, Workspace or Feature connections, and only a Job/attempt/revision-bound structured result submission capability. Profiles requiring unavailable tools/features are explicitly rejected rather than silently stripped. Result authority is independent of Profile name; final prose, Idle and Worker termination are never success evidence. This first generic adapter grants no domain operations; future consumers must provide explicit domain capabilities rather than gaining them through a profile tag.

The Host limits concurrent active resources independently of caller limits, enforces timeout/turn/output limits, routes cancellation into the Internal Worker lifecycle and waits for resource cleanup. Accepted results are committed before resource release; required domain postprocessing must finish before submission (or be independently durable in its consumer). No daemon, pool, subprocess, HTTP/WS listener or Server dependency is introduced.

Validation will distinguish scripted in-process Internal Worker executions and SQLite reopen/recovery fixtures from any real-provider/process tests. It will cover alternative Profiles, immutable retry, invalid/stale results, resource limits, failure/cancellation/timeout, shutdown and restart. Backend focused regression tests must still prove its existing fences and cleanup.

## API and ownership

`job::{JobRequest, JobLimits, JobResultSubmission, JobOutcome}` are the common
Feature-facing data contract. `JobOutcome` distinguishes intent state from attempt
state and omits Workspace/Runtime identities. These enums are read vocabulary,
not another transition engine. Backend converts neutral requests with
`BackendJobRequest::from_job_request` and projects reservation/acceptance with
`.outcome()`; Backend domain grants, provenance, delivery and cleanup remain on
its own adapter and retain their existing durable/wire format and fingerprints.

Standalone consumers receive `StandaloneHost::jobs()` explicitly, not a fake
Workspace client and not a tool on the interactive Worker. They can persist
`request`, read `get`/`pending`, explicitly `start` or `retry`, `cancel`, and
`acknowledge`. `.wait()` observes execution cleanup; `.outcome()` on the returned
snapshot is the common read view. `Completed` with its accepted structured result
is success authority; merely returning from `wait` is not. The injected-client
entry point resolves/validates the same selected Profile and model configuration;
it replaces only the model transport for embedders/tests.

A typical generic consumer supplies its own stable Job ID/revision/reference,
immutable JSON input, instruction and registry Profile:

```rust,ignore
let jobs = host.jobs();
let snapshot = jobs.request(request)?;
if snapshot.state == standalone::jobs::JobState::Pending {
    jobs.start(&snapshot.request.job_id).await?;
}
let outcome = jobs.wait(&snapshot.request.job_id).await?.outcome();
// Validate/apply the consumer's domain result here. A failed notification or
// delivery leaves the successful result unacknowledged; do not retry the model.
if outcome.state == job::JobState::Completed {
    deliver_idempotently(&outcome)?;
    jobs.acknowledge(&outcome.job_id)?;
}
```

The Host retains tasks/cancellation even while a consumer waits; dropping a wait
future does not detach cancellation ownership or release a concurrency slot.
Active resources are bounded to eight by the Host and by the minimum of all live
requests' ceilings, including after result acceptance and before cleanup.
Serialization keys likewise fence live resources, not just pending result state.
Busy `start` returns a typed capacity error and leaves the reserved attempt
unchanged; callers decide when to start/resume pending work. There is no pool,
background daemon, implicit retry or independent Feature-owned Internal Worker.

## Profile and capability policy

The caller must select an existing built-in or user registry Profile. Path/raw
selectors and `inherit` are not accepted. Registry existence, selected instruction
and model configuration are checked before dispatch. The ordinary user settings
registry and Prompt catalog are used; repository-local recipes are not discovered
by the standalone Host, and default never silently maps to a different Profile.

`builtin:job` is an **optional**, compatible result-only policy. It is not a
runner-selected default or authority token. User registry Profiles with different
instructions/models/bounds execute the same way, as demonstrated by the scripted
Worker tests. This adapter deliberately rejects active unsupported Feature,
filesystem/delegation scope, MCP, skills, compaction and durable event-trace
requirements. Existing `builtin:backend-job` has such requirements and is rejected
by this standalone adapter, while its Backend behavior is unchanged. A future
consumer requiring domain operations must arrange explicit Host capabilities;
choosing `builtin:subjektiv-memory-consolidation` alone grants nothing.

Every run uses the existing private Internal Worker substrate, a distinct identity
and ephemeral Session. Only `SubmitJobResult` is installed by a FeatureModule.
Its model input is `{result: ...}`; Job/attempt/input-revision bindings come from
the Host capability, not model arguments. The sink checks those bindings, the
current persisted attempt, deadline, byte limit and result digest. Exact result
replay is idempotent; changed/foreign/stale results are rejected. Submission is
sealed before acceptance, so uncertain responses permit only identical retries.
An ambiguous sink failure is reconciled against SQLite; absent accepted evidence
it is `Unknown`, not a known retryable model failure. No domain logic is embedded
in this sink. Required consumer/domain postprocessing must complete before
submission or have its own durable ownership outside the ephemeral Worker.

Selected instructions are retained. The Host execution contract is rendered from
`resources/prompts/internal/job_system.md` before the run's initial history is
created, not injected into a previous/transient turn. Turn/output policy is
attenuated to at most 32 turns and 8192 output tokens per model request, preserving
tighter selected Profile bounds. Structured results retain the shared 16 KiB
default / 64 KiB absolute byte limits, not a larger standalone exception.

## Persistence and termination

Local schema version **1** is owned only by `standalone::job_store`: `job_intents`
contains immutable serialized intent/fingerprint, serialization key, current
attempt and acknowledgement; `job_attempts` contains immutable identity/revision,
execution state, failure and accepted result/digest. Foreign keys, immutable
identity/result triggers, transactions, `WAL` and `synchronous=FULL` protect the
boundary. No Server schema or subjektiv/Memory schema is copied or opened.
The Host opens and recovers the store only under its existing top-level Worker
lease; clones cannot open or recover it themselves.

- **Exact request replay:** same immutable intent returns its current snapshot.
  Changing any intent field under the same ID is a conflict, not a new attempt.
- **Known failure:** only explicit `retry` creates a new immutable attempt, up to
  the intent's bounded maximum. Prior attempts/results remain in SQLite.
- **Timeout/shutdown before acceptance:** fence `Unknown` before requesting Engine
  cancellation; late result submissions cannot create success. Await normal
  cleanup; retain the Host lease and report an error if confirmation is lost.
- **Normal Host shutdown:** seal scheduling/mutations, cancel active runs, await
  resource cleanup, close SQLite, then stop the interactive controller and release
  its lease. Reserved/unstarted requests remain resumable. Old cloned handles
  return `Closed`, rather than mutating a store reopened by another Host.
- **Unexpected Host drop:** request cancellation and retain the lease because
  synchronous drop cannot attest cleanup. No daemon continues after process exit.
- **Process loss/reopen:** reserved attempts stay pending; unaccepted dispatched
  attempts become unknown; accepted results/acknowledgements remain intact.
  SQLite result evidence, never final prose or an ephemeral Session, decides success.
- **Cancellation:** durable and never automatically resumed. Cancelling already
  unknown intent does not erase its attempt's uncertain execution evidence or
  release its serialization fence. `retry` still refuses it. There is no generic
  operation that declares uncertain domain side effects unexecuted; callers must
  reconcile domain evidence rather than bypassing the fence with an implicit replay.
- **After acceptance:** model error, cancellation or consumer delivery failure
  cannot erase success or rerun the model. Acknowledgement is independently durable
  and idempotent. Execution resources are reclaimed independently of result lifetime.

Emergency cancellation has a bounded grace (one second for cancellation setup,
two seconds for normal cleanup). Expiry is **not** cleanup confirmation: it returns
an error, keeps unknown evidence/accepted results, and the Host retains its lease.
The result-only Feature owns no async/background tasks or domain connections;
the dropped engine stream/tool pumps request abort. Synchronous SQLite submission
is bounded by its five-second busy timeout and cannot be preempted mid-transaction.
No resource or side effect is claimed rolled back merely because a future dropped.

## Validation evidence

See [T-708 validation ledger](../report/2026-10-06-t708-validation.md). It separates
scripted in-process Worker/Host runs, direct SQLite/reopen/rollback fixtures and
an actual test-owned child-process loss/Host restore test from live-provider or
live dogfood validation. T-709's subjektiv consumer is intentionally not migrated
here; Backend T-704 Profile selection and T-690 cleanup remain regression-tested.
