# Session capture and Worker observation

Yoi uses one session-entry exploration domain for host-provided snapshots, Memory extraction evidence, and authorized observation of active Workers. The domain is implemented in `worker::session_capture` and has no Workspace or Memory mutation authority.

## Common capture contract

A host constructs an immutable ordered `SessionCapture` from committed session items. The capture:

- excludes reasoning before overview, search, read, or evidence projection;
- does not include the session system-prompt field;
- assigns an append-stable `SessionEntryRef` (`E...`) from the committed source position;
- uses the same reference in sparse overview anchors, search results, bounded reads, and Memory evidence conversion;
- pages sparse real user/assistant anchors and reports the number of non-reasoning entries between anchors;
- supports bounded range search and compact range listing when no filter is supplied;
- bounds read item count and bytes.

`SessionEntryRef` is local to the selected session subject. Runtime peers use the canonical `{ kind: "runtime_worker", runtime_id, worker_id }` reference; parent-owned children use `{ kind: "sub_worker", name }`. A model must first select a host-projected session and must reuse both the structured subject and entry references returned for that session.

## Independent features

The three feature modules share only the capture domain:

- `session-explore` installs `ShowOverview`, `SearchEntries`, and `ReadEntry` for one immutable host snapshot. It has no Workspace client or Memory state.
- `memory-extract` installs `StageMemoryCandidate` and `FinishMemoryExtraction`. It validates every staged `entry_ref` against its co-installed capture before converting it to typed Memory evidence.
- `worker-observation` installs `ListWorkerSessions`, `ViewSessionOverview`, `SearchSessionEntries`, and `ReadSessionEntry`. It captures the selected Worker again on every operation, so newly committed entries become visible while existing append-only references remain stable.

The features do not enable or mutate each other. Feature-registry collision checks remain authoritative for tool names.

## Observation authority

`WorkerObservationProvider` is a host-injected authority boundary. The Worker receives only an `Arc<dyn WorkerObservationProvider>`; model input never supplies grants, Workspace credentials, Runtime URLs, session handles, repository paths, or provider clients.

The provider must:

1. list only active subjects already granted to the current Worker;
2. reauthorize every capture instead of trusting a previous list result;
3. return the same not-found result for missing and unauthorized subjects;
4. return only committed session items;
5. keep subject identifiers opaque and bounded.

Runtime/Backend integrations enable the feature through the Backend-only `WorkerSpawnRequest.resolved_worker_observation_enabled` field, forwarded as `CreateWorkerRequest.worker_observation_enabled`. Canonical same-Runtime peers may also be supplied through `resolved_worker_observation_grants`; Runtime revalidates those against live weak handles and Workspace scope. For cross-Runtime peers and dynamically added Workers, `WorkspaceClientWorkerObservationProvider` calls the Workspace-scoped Server projection on every list/capture. Server authorizes that route against the current Workspace Orchestrator identity, recomputes the active Workspace Worker set, and reads the selected Worker’s committed protocol snapshot. Runtime binds these providers through `Worker::bind_worker_observation_provider` on spawn and restore. Parent-owned SubWorkers use the same provider contract through `SpawnedSubWorkerObservationProvider`; their subjects use the tagged `sub_worker` variant.

Observation is read-only evidence access. It does not authorize Ticket, Memory, Worker, or Workdir mutations and is not completion or approval authority.

## SubWorker output

SubWorkers no longer expose a separate output cursor tool. `SubWorkerList`, `SubWorkerSend`, and `SubWorkerStop` retain parent-owned lifecycle control, while committed child output is read through `worker-observation`. Turn-completion notifications carry no transcript and only tell the parent to inspect the authoritative committed session at a natural boundary.
