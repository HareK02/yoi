# Workspace Worker operations

T-702 documents the implemented Workspace operation-reception boundary. It does not move execution authority into the Workspace registry or make all Worker-related routes use one API. The common implementation is [`worker_operations.rs`](../../crates/workspace-server/src/server/worker_operations.rs); production adapters and service orchestration are in [`server.rs`](../../crates/workspace-server/src/server.rs).

## Identity reception is not execution authority

`WorkspaceWorker::resolve` resolves a Workspace registry reference to a `RuntimeWorkerRef` (Runtime ID and Worker ID). Resolution first tries that exact pair, then a Workspace resource reference constrained to the requested Runtime. It returns an identity-bound handle, not a live execution snapshot, authorization lease, or promise that the Worker is running. Each operation rereads the target registry record and authorizes its own context.

The owners have different responsibilities:

- **Workspace registry and domain services:** durable Workspace identity, retention/pinning, Workdir links and sessions, Ticket assignments, credentials/bindings, control grants, restore coordination, and removal plans/catalog publication. Registry projections and observation streams describe Workers; they do not admit controller commands.
- **`RuntimeRegistry` and Runtime adapters:** select the Runtime owning the resolved pair and delegate execution. Remote and embedded adapters preserve their existing transport/error behavior. Runtime owns lifecycle operation settlement and checks Workspace scope at the scoped embedded execution boundary.
- **Execution backend and Worker controller/session:** own the actual execution endpoint, command admission, authoritative Idle/Busy state, in-flight run transitions, Submit FIFO and durable receipts, Notify receipts, and compaction. A catalog status is not sufficient evidence for command admission.

The term *controller* in a `WorkerControl` context means the calling Runtime Worker holding a grant over a target. It is not the target's in-process execution controller.

## Production ingress → common method → execution owner

For the HTTP rows below, `R` abbreviates `/api/runtimes/{runtime_id}/workers/{worker_id}` and `W` abbreviates `/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}`. Aliases operate in the server's configured Workspace; scoped routes additionally validate the Workspace path. The declarations live in [`server-api`](../../crates/server-api/src/lib.rs), and the generated contract service implementations in `server.rs` are the production unary adapters.

| Ingress | Common reception / coordination | Execution or persistence owner |
| --- | --- | --- |
| `POST R/restore` and `POST W/restore` | `WorkerOperationContext::from_request` → `restore_runtime_worker_with_context` → `WorkspaceWorker::restore`; scoped route first uses `scoped_restore_runtime_worker_with_context` | `WorkspaceApi::restore_workspace_worker` retains the existing singleton, Workdir, credential/binding and pending-operation reconciliation flow; Runtime/backend owns restored execution |
| `POST R/input` and `POST W/input` | Resolve → convert request → `WorkspaceWorker::input` | `RuntimeRegistry::send_input` → Runtime adapter → execution backend/controller/session |
| `POST R/stop` and `POST W/stop` | `WorkspaceWorker::stop` | Runtime owns Stop; on an **Accepted** result Workspace closes the current Workdir session and synchronizes linked Workdir state. Rejected/Unsupported results do not perform that finalization |
| `POST R/cancel` and `POST W/cancel` | `WorkspaceWorker::cancel` | `RuntimeRegistry::cancel_worker` → target execution controller; not registry removal |
| `PUT W/pin` / `DELETE W/pin` | `WorkspaceWorker::set_pinned(true/false)` | Workspace store retention update, ordered through `worker_projection`; no execution command |
| `GET /api/w/{workspace_id}/runtimes/{runtime_id}/cleanup-plan` | `build_runtime_cleanup_plan` (preview, not a Worker mutation) | Workspace cleanup planner/projections |
| `POST /api/w/{workspace_id}/runtimes/{runtime_id}/cleanup-executions` | `execute_runtime_cleanup_with_context` validates plan digest and selected candidates → each selected Worker's `delete_from_plan` | `WorkerRemovalService::execute_cleanup_removal` → existing retention planning, Runtime retention execution, and Workspace catalog commit. Workdir-only actions remain in the cleanup service |
| `POST /api/w/{workspace_id}/workers/remove` (`WorkerRemove`, not a browser removal alias) | `worker_remove_contract` verifies its source proof against the requested target → `WorkspaceWorker::remove` | `WorkerRemovalService::execute_async` checks the `remove` grant and coordinates retention/removal. It rejects self-removal and requires a stopped target for a new removal; existing durable removal operations have their own recovery path |
| Embedded `WorkerRemove` Tool | Runtime-owned in-process target-bound proof → `EmbeddedServerWorkerMutationDispatcher` verifies/consumes proof → installed `EmbeddedWorkspaceWorkerRemoveExecutor` → `WorkspaceWorker::resolve_with_services` → the same `WorkspaceWorker::remove` | Same `WorkerRemovalService::execute_async`, grant-lock/recheck, self/live/retention guards, durable plan/recovery and catalog commit as HTTP; no Runtime-to-Server dependency |
| Dedicated WS `R/protocol/ws` and `W/protocol/ws` | `worker_protocol_ws` (scoped wrapper for `W`) → `WorkspaceWorker::connect_protocol` → `workspace_worker_protocol_session` → bound sender | One remote WS or embedded execution transport; controller/session owns protocol operation results |
| Multiplexed WS `/api/w/{workspace_id}/protocol/ws`, runtime-scoped `WorkerProtocol` selector | `scoped_workspace_protocol_ws` → `serve_workspace_subscription` → resolve + `connect_protocol`; subscription stores the bound sender | Same connected execution transport as dedicated WS; the subscription layer wraps its snapshot/events, not a different execution implementation |
| Worker-control `POST /api/w/{workspace_id}/worker-control/workers/{runtime_id}/{worker_id}/{input,cancel,stop,restore}` | `send_known_worker_input`, `cancel_known_worker`, `stop_known_worker`, `restore_known_worker` authenticate the Worker source, construct `WorkerControl`, and call the same identity methods/restore adapter | Same Runtime execution owners, under the applicable active target grant; known-worker input adds its existing non-Accepted-result error translation |

Both WS adapters call `WorkspaceWorkerMethodSender::send`, which selects the following **named methods** rather than implementing separate command semantics in each transport:

| Public protocol method | Named common sender method | Distinct owner/receipt |
| --- | --- | --- |
| `Pause { command }` | `pause` | Controller command admission/acknowledgement; pause is not Stop |
| `Resume { command }` | `resume` | Controller resumes the paused run; not Restore |
| `Cancel { command }` | `cancel` | Controller command admission for cancelling the current run; distinct from HTTP lifecycle request/result |
| `Compact { command }` | `compact` | Controller maintenance/compaction, with command acknowledgement and subsequent lifecycle events |
| `Submit { submission_request_id, input }` | `submit` | Session durable Submit receipt/FIFO and later run execution |
| `Notify { notification_request_id, message }` | `notify` | Session durable Notify receipt and advisory delivery, not the Submit FIFO |

All six methods funnel through `dispatch` / `send_checked`. Other browser-allowed protocol methods also use that checked dispatch; the six named methods are not an exhaustive allowlist. For example pending-submission operations, rewind, Shutdown, completions, ListWorkers, RestoreWorker and RegisterPeer are allowed by `authorize_browser_worker_method`. This forwarding does not replace their lower-layer semantics with Workspace registry operations.

`build_inner_router` installs the three production WS routes using typed `websocket_route` registrations and merges the generated Workspace contract router for unary routes. [`workspace_subscription.rs`](../../crates/workspace-server/src/workspace_subscription.rs) handles multiplexing. Its `WorkspaceWorkers` subscription is a separate catalog stream with no method sender; only a runtime-scoped `WorkerProtocol` subscription carries an operational sender.

## Context and authorization restrictions

`WorkerOperationContext` is crate-private. Clients cannot submit this enum to select their authority.

- **Request-derived Worker control:** `from_request` uses the middleware-verified Runtime Worker subject and current Workspace registry membership. The shared source-binding helper rejects Runtime-only proofs and conflicting identity hints; no hint-only request or Account fallback can impersonate a Worker. Known-worker adapters bind their source through the same verified request extension before constructing the operation context.
- **Account/browser context:** despite the `Browser` name, this also represents account-authenticated unary requests. It retains request headers and the server-derived Account input source. On each common authorization call, the server re-resolves credentials, rejects revoked credentials or an Account change, requires the configured Origin for `BrowserSession` actors, and requires an active Workspace. It does not interpret the operation's permission string as a Worker-control grant check for an Account actor; request middleware remains part of the route's authorization boundary.
- **Worker-control context:** the target and calling Worker must be registered. The server requires an active grant for this controller/target pair and the operation's permission, locks by grant ID, then rereads the target and rechecks permission and the exact grant ID after acquiring the lock. A replacement grant requires new admission under its own lock. A missing active grant is reported as UnknownWorker; a present grant lacking the permission is InvalidInput. Merely knowing an ID does not grant control.
- **Test Backend context:** `WorkerOperationContext::Backend` exists only under `#[cfg(test)]`. It bypasses context authorization after the target-record check. There is no production Backend enum variant for a client, Worker Tool, or generic internal caller to select. Production Backend services continue to own their explicit domain flows.

Worker-control grants are Workspace-owned records, not inferred parentage or universal access to every Runtime Worker. The production spawn flow persists a controller/subject grant with `send_input`, `notify`, `cancel`, `stop`, `restore`, `remove`, and `observe`; that default does not include `pin`. Known-worker discovery follows active grants, while Internal children use their separate parent-owned registry. Other grant origins/permission sets must be read from their own authority rather than assumed from this spawn default.

Permissions are `restore`, `stop`, `cancel`, `pin`, and `remove` for those identity operations. HTTP input uses `notify` for Notify and `send_input` for all other accepted kinds. `connect_protocol` checks `observe`; every connected method dispatch checks `send_input`, requires an interactive target, then explicitly requires a **Browser** context. Thus an observe grant is not a mutation grant, and a Worker-control grant does not enable browser WS method forwarding. WS Notify also uses this browser-only `send_input` check, not HTTP input's `notify` grant mapping.

`input` and all connected protocol method sends reject Workers with a Backend Job projection: their immutable input is Backend-owned and their Console is read-only. Connecting for observation is not itself rejected by that interactive check.

For Worker-control user input, `input` converts `User` to `UserIfIdle`; admission belongs to the target controller and never queues behind an active run. Browser Submit may durably queue while Running/Paused. Notify is advisory input, not a queued Submit; it can be incorporated at a between-turn boundary or drive the notification flow while Idle. The known-worker input adapter turns definite non-Accepted input results into errors, but keeps uncertain outcomes as the existing typed result with a `worker_input_outcome_unknown` diagnostic, and supplies the Idle-only guidance for user input.

Scoped Restore may coordinate Ticket assignment only when both `ticket_id` and `assignment_operation_id` are supplied. A Worker-control context cannot use either assignment parameter. Aggregate cleanup rejects all Worker-control requests: Worker callers must retain the stronger target-bound WorkerRemove proof/retention path. A control grant is not authority over Ticket assignments or Workdir cleanup. The cleanup coordinator revalidates Account credentials, Origin, and active Workspace even for a Workdir-only request.

The HTTP and embedded `WorkerRemove` entry points share the registry-bound removal method. `WorkerRemove` intentionally has a distinct authorization contract from the other operations. It accepts a verified mutation source rather than `WorkerOperationContext`, verifies its target-bound proof before calling the identity method, and keeps source-specific and grant revalidation in the removal service. It is not permission-free just because it does not call `authorize_operation`.

The embedded executor retains only the removal services with a **weak** RuntimeRegistry reference. Its removal-only `WorkspaceWorker<()>` specialization shares resolution, current-record checks and the removal method with the full `WorkspaceWorker<WorkspaceApi>` handle. This service projection avoids a strong `WorkspaceApi → Runtime → dispatcher → WorkspaceApi` ownership cycle; it is not another Worker ledger or an authorization fallback. It cannot call the full handle's restore/input/protocol methods. The existing independent Runtime-owned proof forwarder and verification/replay seam stay unchanged.

## Tokens and outcomes are operation-specific

There is no universal Worker command token or universal success result.

- `RuntimeWorkerRef` identifies the registry target; a Workspace resource reference is an ingress lookup form. Neither identifies a particular run or proves authority.
- `WorkerCommandEnvelope.command_id` orders controller lifecycle commands. `WorkerCommandAcknowledgement` contains command kind, disposition (`Accepted`, `StaleCommandId`, `Conflict`, `InvalidState`) and authoritative state. These IDs belong to controller command admission, not durable Submit identity or Workspace removal planning.
- `submission_request_id` is the caller's Submit retry key in an authenticated-source namespace. A durable `SubmissionAccepted` receipt also contains the Worker-allocated `submission_id` and `Started`/`Queued` disposition. `SubmissionRejected` is correlated by request ID. Acceptance is not a successful completed run; `last_finished_submission_request_id` is a teardown fence, not a result-success claim.
- `notification_request_id` correlates `NotificationAccepted` / `NotificationRejected`. Notify has its own durable receipt and exact-payload retry behavior; it does not allocate a Submit FIFO entry. Browser Submit/Notify are rewritten to `SubmitTracked`/`NotifyTracked` with the server's Account source. Browser-supplied tracked forms, `SubmitIfIdle`, and `WorkerEvent` are rejected.
- Runtime `WorkerLifecycleOperationId` is a durable lifecycle/reconciliation identity, separate from a controller command ID. Ticket `assignment_operation_id`, cleanup plan digest, and retention plan/operation IDs and fingerprints likewise belong to their own domain checks. A multiplexed `subscription_id` only routes frames to an already-bound sender.

The common sender returns `Result<()>` after authorization and enqueueing on its transport channel. This is **not** a controller acknowledgement, durable receipt, or proof of upstream delivery. Actual outcomes arrive on the event stream. The dedicated WS reports dispatch errors as protocol error events; the multiplexed adapter currently terminates the connection when sending a method fails. It also requires an initial session Snapshot before accepting the WorkerProtocol subscription.

Unary input/lifecycle responses preserve their existing API projections: `WorkerOperationState` is `Accepted`, `Unsupported`, or `Rejected`, with Runtime/Worker IDs and diagnostics. Restore instead has `Accepted`, `Rejected`, `RolledBack`, and `ReconciliationRequired`. Pin returns retention state; cleanup returns per-target actions/statuses; WorkerRemove has its own response/error body. These are not interchangeable acknowledgements.

A transport or store failure can leave an **unknown outcome** after effects have started. Lower-layer input results have `WorkerInputDisposition::{Accepted,Rejected,Unknown}` and an optional `runtime_run_id`, but `worker_input_result_to_api` projects only state, target IDs and diagnostics: the public unary input result does not expose those two fields. Unknown delivery is explicitly preserved as a `worker_input_outcome_unknown` diagnostic; neither HTTP adapter changes it into definite rejection or retries it. Runtime execution results also distinguish Busy/Rejected/Errored/Unsupported; error handling and durable restore/removal recovery remain lower-layer responsibilities. Do not infer that a rejected-looking projected response or a closed WS proves that nothing was accepted. The new reception layer does not add automatic replay or an exactly-once guarantee across every ingress. In particular, `worker_input_request_from_api` sets `submission_request_id` to `None`; the unary Workspace input contract does not provide the browser WS Submit retry-key contract.

## Connected execution lifetime

`WorkspaceWorkerMethodSender` stores both the resolved Worker handle and the sending channel privately. A WS caller cannot pair a different Worker handle with a subscription's raw sending capability. Registry identity can outlive an execution, but an operational connection does not follow it to a replacement execution.

`connect_workspace_worker_protocol` obtains an observation source from the Workspace proxy (falling back to the Runtime only for WorkerNotFound). It then connects to one remote execution WS or attaches one embedded `WorkerProtocolTransport`. The embedded path captures the real endpoint and its initial snapshot, not the WorkerRef-wide catalog observation bus.

Embedded dispatch uses `send_connected_protocol_method_scoped`. Runtime verifies that the transport target equals the authorized WorkerRef, checks scope and pending-operation settlement, and validates the captured endpoint before any durable mutation. Even a delayed terminal-close observation cannot make the old endpoint Stop or send to a new execution under the same identity.

Shutdown and a closed execution relay terminate the operational stream. Embedded lifecycle/store/transport uncertainty is terminal; a settled Busy/Rejected/Unsupported rejection may leave it usable only if the endpoint still validates. Remote bridge failure closes its channels. `send_checked` reports `worker_protocol_closed` when its channel is closed and explicitly states that no implicit Restore or retry occurred. No bridge in this path retargets, restores, or reconnects to the next execution. The caller must explicitly Restore when authorized and establish a new connection; this is different from catalog observation continuing to describe the same registry identity.

## Internal Workers are parent-owned

Internal SubWorkers and service Internal Workers are not Workspace registry targets or independently addressable Runtime protocol subjects. They remain separate, parent-owned sessions. Their visible identity is `InternalWorkerRef.session_id` (`name` is display-only), and snapshots/events are presented through the parent's protocol stream; service-private Internal Workers are excluded from the public child inventory.

Direct-child control/observation stays in the parent-owned Worker machinery. A Workspace Runtime Worker grant is not a grant to an arbitrary Internal Worker, and this registry handle does not create an Internal Worker Runtime record, independent transport, or implicit Restore path. The Workspace observation capture adapter explicitly rejects a non-RuntimeWorker subject; it does not resolve a SubWorker as a registry Worker.

## Regression helpers are not production routes

Lower-level test functions in `server.rs` must not be mistaken for installed unauthenticated endpoints:

- `set_worker_retention`, `send_runtime_worker_input` / `dispatch_runtime_worker_input`, and `execute_runtime_cleanup` / `scoped_execute_runtime_cleanup` are `#[cfg(test)]` wrappers using the test-only Backend context.
- `worker_protocol_ws_session`, `remote_worker_protocol_ws_session`, and `embedded_worker_protocol_ws_session` are `#[cfg(test)]` transport regression helpers. Production dedicated WS uses `workspace_worker_protocol_session` with the common bound sender; production multiplexed WS uses `serve_workspace_subscription` with that same sender type.
- Production `connect_workspace_worker_protocol`, `connect_remote_worker_protocol`, and `connect_embedded_worker_protocol` are real shared connection helpers, not those test-only sessions. The grant-checked observation capture service also calls the connection helper to read a committed Snapshot; that read-only capture is outside the six operational WS methods.

Source presence alone is not route wiring. Follow `build_inner_router`, generated contract implementations, and the typed route declarations when auditing production ingress.

## Minimum scope, limitations, and future guard

The boundary above covers the T-702 minimum operation set. It is not a claim that every session/file/completion API has been migrated:

- Session snapshot/history and retained session attachment reads remain their existing observation/retention paths. They are not controller command admission.
- Attachment upload grants and upload cancellation keep their Account/target-bound grant checks; file upload and uploaded-file deletion still resolve the target and delegate through `api.runtime`, rather than calling a new `WorkspaceWorker` file method.
- Unary completions still resolve the registry reference and call `api.runtime.worker_completions`. Protocol `ListCompletions` is also browser-allowed through checked WS dispatch. Those are different ingress paths.
- Worker creation, Backend Job execution, rollback compensation, Workspace deletion, and other domain-owned orchestration are not universally rewritten to this handle. Legitimate lower-level Runtime calls still exist.

Known limitations worth follow-up, rather than undocumented guarantees:

1. Browser credentials/Account are rechecked for each common operation or WS method send, but not continuously for every outgoing observation event. This is not a promise of immediate revocation-driven stream teardown.
2. Cleanup is sequential rather than an all-target atomic transaction. The existing cleanup plan digest and each removal service's live revalidation remain distinct protections.
3. Unary input has its existing narrower wire contract, rather than the WS authenticated receipt envelope. Identity unification does not create a new retry contract or manufacture caller request IDs across transports.

T-701's [stale-observation Restore guard](worker-restore-observations.md) connects the caller's `expected_observation_token` and `request_id` to Runtime's existing operation lock, distinguishes a new Restore intent from pending reconciliation, and handles UI conflicts. This boundary leaves those Restore inputs/results and the pending-operation state machine intact rather than creating a duplicate guard.

### Implementation references

- [`server/worker_operations.rs`](../../crates/workspace-server/src/server/worker_operations.rs): identity handle, contexts, authorization, named methods and private bound sender.
- [`server.rs`](../../crates/workspace-server/src/server.rs): contract adapters, `build_inner_router`, scoped Restore/cleanup, WorkerRemovalService, dedicated WS and shared transport bridges.
- [`workspace_subscription.rs`](../../crates/workspace-server/src/workspace_subscription.rs): multiplexed WorkerProtocol binding and catalog subscriptions.
- [`hosts.rs`](../../crates/workspace-server/src/hosts.rs): Runtime registry/adapters and distinct input/lifecycle result projections.
- [`protocol`](../../crates/protocol/src/lib.rs): command envelopes/acknowledgements, Submit/Notify receipts and Internal Worker presentation.
- [`worker-runtime/runtime.rs`](../../crates/worker-runtime/src/runtime.rs) and [`execution.rs`](../../crates/worker-runtime/src/execution.rs): scoped connected endpoint validation, lifecycle identities and execution results.
