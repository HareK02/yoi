# Durable side-effect operations

A durable side-effect operation is a Backend-owned intent whose execution crosses an authority boundary, such as a Runtime/provider mutation, and must converge across duplicate requests and bounded recovery. The durable record is not a trace of Rust control flow.

## Durable authority

The record stores only facts that affect identity, authorization, replay, or the final domain result:

- a stable operation identity and request fingerprint derived from caller intent;
- the Workspace resource and the resolved authority that exact retries must keep;
- `pending`, `failed`, or `completed` lifecycle state;
- attempt count and timestamps as operational evidence;
- explicit retryability, bounded failure category, and bounded disposition;
- a factual checkpoint only when a non-idempotent provider effect cannot be safely re-observed or repeated.

A fingerprint excludes Server-generated result identifiers, attempt data, diagnostics, and fresh observations. Reusing one operation identity with a different fingerprint is an error. A completed exact retry replays the committed bounded result.

`pending` means only that the intent remains open. Function names, validation steps, and provider-call positions are not persisted as lifecycle stages. `failed` records the latest terminal attempt outcome; retryability remains separate metadata. `completed` means the required domain result and disposition are durably committed.

## Live authority and provider evidence

Before every attempt, the Backend rereads current Workspace ownership and guards. A previous observation that a resource was detached, unblocked, or clean is not authorization for a later retry.

Provider timeout, unavailability, an empty response, or an unknown outcome is not authoritative absence. Registry cleanup may use only an explicit provider success contract or the provider's exact not-found evidence. If a provider effect is idempotent and exact not-found can be re-observed, arbitrary execution stages and crash-window checkpoints are unnecessary: recovery repeats observation and converges from last committed facts.

## Workdir removal

Workdir removal is one durable side-effect operation in the Workspace Server DB. It binds the Workspace, Workdir, owning Runtime, Repository/materialization identity, source actor, stable intent fingerprint, lifecycle, retry metadata, and bounded result. Runtime URL, provider handle, host path, credentials, and caller-selected Runtime are not operation inputs.

Each attempt:

1. resolves or revalidates the persisted same-Workspace Workdir, Runtime, Repository, and materialization identity;
2. checks current attachments, attachment reservations, current assignment occupancy, retention/cleanup holds, and pending materialization authority;
3. retains dirty, occupied, blocked, or otherwise unknown Workdirs without detaching a Worker or forcing deletion;
4. observes the owning Runtime/provider and calls its existing Workdir cleanup only for an eligible clean Workdir;
5. treats only successful provider cleanup or exact `working_directory_not_found` as removal evidence;
6. deletes the Backend Workdir registry row and commits the operation's `completed`/`removed` result in one SQLite transaction.

A provider error leaves the registry intact and records a bounded `attention_required` result with explicit retryability. Startup recovery lists `pending` and retryable `failed` operations, then executes this same path after rereading live authority. `WorkdirDelete`, Workspace REST removal, Runtime cleanup execution, and recovery must not maintain separate inline provider-delete paths.

The public request contains only `working_directory_id` plus a bounded reason. The public result contains only the Workdir ID, `removed | retained | attention_required`, retryability, and an optional bounded failure category. Internal operation identifiers, checkpoints, provider paths, and credentials are not public DTO fields.

## Resilience boundary

This pattern covers duplicate requests, returned failures, timeouts and unknown outcomes, known partial-completion contracts, and restart recovery from committed facts. It does not provide general exactly-once execution or claim recovery from every instruction-boundary panic, process kill, machine loss, or power failure. A stronger provider guarantee requires a separately specified protocol, checkpoint ordering, reconciliation evidence, and tests.
