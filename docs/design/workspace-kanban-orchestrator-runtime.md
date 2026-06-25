# Workspace Kanban to backend Orchestrator runtime

Workspace Kanban operations are control-plane requests. They may change Ticket state and request orchestration, but they must not directly execute shell, git, filesystem work, or send authority-bearing messages to raw local Worker sockets. The durable boundary is an orchestration event consumed by a backend-internal Orchestrator Worker.

This document records the design boundary for connecting Kanban operations, Tickets, the Workspace backend, `WorkerRuntimeRegistry`, and filesystem-capable Workers. It is intentionally a planning artifact: it does not require the Workspace backend to implement every table, API, remote protocol, or spawn adapter immediately.

## Core rule

A browser click or API request can create durable intent; it cannot be the authority to perform implementation side effects.

The minimum chain for implementation work is:

1. A user or authorized caller changes a Ticket through the Workspace/Ticket API.
2. The state change and its orchestration event are committed durably in the same logical operation.
3. A backend-internal Orchestrator Worker reads the event with domain-specific tools.
4. The Orchestrator records a routing decision.
5. If the Ticket is queued, unblocked, and accepted for implementation, the Orchestrator records `queued -> inprogress` plus the acceptance decision before any implementation side effect.
6. The Orchestrator creates typed spawn intents for filesystem-capable Coder, Reviewer, or helper Workers.
7. A local or remote runtime adapter resolves those intents into concrete worker launch/configuration using its own authority and capability policy.

`ready -> queued` is therefore a human gate for Orchestrator routing, not an unattended scheduler and not a lease that automatically starts code execution.

## Durable orchestration events

Orchestration events are immutable control-plane records derived from Ticket operations. They are not raw LLM messages, Worker notifications, or socket writes.

Initial event kinds:

- `ticket_queued`: emitted for `ready -> queued`; requests Orchestrator routing/start-if-unblocked checks.
- `ticket_state_changed`: emitted for other lifecycle transitions that may affect orchestration state.
- `ticket_returned_to_planning`: emitted when a Ticket is moved back to `planning` because concrete requirements, decisions, dependencies, or acceptance evidence are missing.
- `ticket_done`: emitted when implementation/review flow records `done`; used for completion projection and close-readiness, not implicit close.

A stable event shape should include:

```text
orchestration_event {
  event_id: opaque id,                         # durable event identity
  workspace_id: opaque workspace id,
  ticket_id: canonical Ticket id,
  kind: ticket_queued | ticket_state_changed | ticket_returned_to_planning | ticket_done,
  before_state: optional Ticket state,
  after_state: optional Ticket state,
  actor: { kind, key, display, source? },      # human, worker, api client, system
  source: { kind, surface, operation },        # kanban_api, ticket_api, orchestrator, import, ...
  request_id: opaque idempotency/correlation id,
  caused_by_event_id: optional event id,
  occurred_at: timestamp from the authoritative mutation,
  recorded_at: timestamp when stored,
  body: bounded structured reason/summary,     # no raw transcript or raw local path authority
}
```

`request_id` is a correlation and idempotency key. For API-originated state changes, retrying the same request must return or reference the already-created Ticket transition and orchestration event instead of creating a second routing command. The event store should reject duplicate `(source, request_id)` pairs for mutation-producing requests, while allowing new events for distinct state changes.

The Ticket state mutation and event append must be atomic from the backend's perspective. If the Ticket changes but the event cannot be recorded, routing must be considered failed and visible; if an event exists, the Orchestrator must be able to recover it after backend restart.

## Event processing, retry, ack, defer, fail

Event delivery state is separate from the immutable event. A future implementation can store per-consumer delivery rows such as `(event_id, consumer_id, status, attempts, visible_after, last_error, updated_at)`. That delivery mechanism is only for reliable control-plane processing; it must not redefine `queued` as a scheduler state.

The backend-internal Orchestrator handles each event idempotently:

- Reload the current Ticket, relations, orchestration plan, worker links, and relevant runtime summaries before deciding.
- Treat stale events as evidence, not commands. For example, a `ticket_queued` event for a Ticket that is now `planning` should be acknowledged with a stale/no-op decision rather than spawning work.
- Record a decision, waiting reason, or failure summary before acknowledging a routing event.
- Create spawn intents with stable `intent_id`s so dispatch retry can detect already-created intents.

Delivery outcomes:

- `ack`: the event has been interpreted and its durable outcome is recorded. The outcome may be `no_op_stale`, `routed_to_worker`, `blocked_waiting`, `returned_to_planning`, or `completion_projected`.
- `defer`: the event is valid but cannot progress now. The Orchestrator records a waiting reason and optional retry condition/backoff. Defer is appropriate for dependency not done, target conflict, dirty workspace reported by a capable runtime, missing worker capacity, or runtime unavailable.
- `fail`: processing cannot safely continue without operator or developer intervention. Fail is appropriate for invariant violations, malformed event payloads, missing Ticket authority, identity ambiguity, repeated dispatch inconsistency, or a request that would require forbidden authority.

Retries must be bounded and idempotent. A retry may re-read the Ticket and registry and either dispatch a previously recorded intent, update a waiting reason, or fail with escalation. It must not execute shell/git/filesystem work in the backend process to "check" whether progress is possible.

## Backend-internal Orchestrator Worker

The backend-internal Orchestrator is a routing/control-plane Worker. It can be hosted inside the Workspace backend runtime because its tools operate on domain records and runtime registry abstractions rather than the workspace filesystem.

Responsibilities:

- Read durable orchestration events and delivery state.
- Inspect Tickets, Ticket relations, accepted orchestration plans, worker links, and bounded project-record projections.
- Decide whether a queued Ticket is ready for implementation, should wait, should return to planning, or should request review/closure follow-up.
- Record decisions, audit summaries, blocker/waiting reasons, and orchestration plan artifacts.
- Select a runtime by required capabilities.
- Create and dispatch typed spawn intents through `WorkerRuntimeRegistry`.
- Append/read worker run overviews, lifecycle summaries, and usage aggregates.
- Escalate when authority, requirements, or runtime capabilities are insufficient.

Non-responsibilities:

- No `Bash` authority.
- No raw workspace `Read`/`Write`/`Edit` authority.
- No direct git/worktree/build execution.
- No raw local Worker socket or session path authority.
- No use of browser-supplied local paths, executable paths, runtime registry paths, `display_ref`, `worker_name`, or runtime display names as operation authority.
- No raw session transcript full ingest into the Workspace database.
- No permission/auth, remote runtime protocol, Ticket DB migration, Kanban UI completion, or Coder/Reviewer spawn implementation completion in this design step.

If routing needs evidence that only filesystem access can provide, the Orchestrator records a helper spawn intent for a filesystem-capable runtime or records a waiting/escalation reason. It does not temporarily grant itself filesystem tools.

## Domain-specific tool surface

The internal Orchestrator should receive backend tools, not generic Worker tools. The tools should be narrow enough to enforce lifecycle and authority rules and broad enough to let future Orchestrator prompts reason without hidden context injection.

Required operation groups:

- Ticket operations:
  - list Tickets by state/risk/assignment bounds;
  - show Ticket details, thread summaries, and relevant artifacts;
  - append Ticket comments/implementation reports/review notes;
  - perform validated state transitions, including `queued -> inprogress`, return-to-planning, and `done` recording.
- Relation and plan operations:
  - read typed Ticket relations and derived blockers;
  - read/write bounded orchestration plan artifacts;
  - record conflict/capacity/waiting/accepted-plan notes.
- Event delivery operations:
  - read pending orchestration events;
  - ack, defer, or fail events with durable reason codes;
  - query retry/defer state by event, Ticket, or consumer.
- Runtime registry operations:
  - list runtimes and capability summaries;
  - look up Workers by canonical `runtime_id` + `worker_id`;
  - query capability support such as backend-internal tools, filesystem, shell, git, worktrees, bounded transcript read, stream availability, and worker spawn support.
- Spawn intent operations:
  - create spawn intents;
  - dispatch intents to a selected runtime;
  - read intent state and acceptance evidence;
  - associate intent/Worker summaries with Tickets.
- Worker overview and usage operations:
  - append/read run overview entries;
  - append/read lifecycle summaries;
  - read usage aggregates for dashboard/control-plane display.
- Audit and decision operations:
  - append routing decisions with actor/source/request id;
  - append authority-boundary failures and escalation requests;
  - query bounded decision history for a Ticket.

Forbidden operation groups for the internal Orchestrator:

- shell execution;
- raw filesystem read/write/edit over repository paths;
- raw Unix socket connects or socket path notification;
- raw session full transcript ingest;
- local Worker metadata path or session path access as authority;
- browser-provided display labels, paths, or executable strings as authority.

## WorkerRuntime registry and spawn intents

`WorkerRuntimeRegistry` is the boundary between backend control-plane decisions and runtime-specific worker launch. The Orchestrator asks for capabilities and submits typed intents; it does not construct low-level process commands.

A spawn intent should describe policy and purpose rather than launch mechanics:

```text
worker_spawn_intent {
  intent_id: opaque id,
  parent_event_id: orchestration event id,
  request_id: idempotency/correlation id,
  ticket_id: canonical Ticket id,
  role: intake | orchestrator | coder | reviewer | helper,
  purpose: route | implement | review | inspect | validate | summarize,
  required_capabilities: [backend_internal_tools | workspace_fs | shell | git | worktrees | build | bounded_transcript],
  workspace: { workspace_id, repository_targets? },
  cwd_semantics: role_default | ticket_worktree | target_repository | runtime_resolved,
  profile_intent: builtin role/profile selector intent,
  workflow_intent: optional workflow slug/phase intent,
  input_packet_ref: durable bounded context reference,
  acceptance_requirement: socket_ready | run_accepted(expected_segments) | decision_recorded,
}
```

The browser must not provide raw workspace roots, child cwd, executable paths, raw profile files, socket paths, local Worker names, or runtime display names in this intent. API callers can request high-level operations such as "queue this Ticket" or "open this canonical Worker"; the backend and runtime adapters resolve launch details from trusted workspace records, runtime configuration, and capability policy.

Runtime adapters are responsible for translating an accepted intent:

- A backend-internal runtime may create routing-only/intake/dashboard-assistant Workers with backend tools and no filesystem scope.
- A local Worker runtime may resolve a Coder/Reviewer intent into Worker launch arguments, scope, delegated filesystem paths, branch/worktree policy, prompt/profile/workflow selection, and acceptance evidence.
- A remote runtime may perform the same adaptation on a different machine without exposing local paths to the browser or storing them as API authority.

Dispatch success means the runtime accepted the typed intent and returned durable acceptance evidence. It does not by itself prove the Ticket is done. Worker progress is projected through lifecycle, overview, review, and Ticket state records.

## Runtime selection by capability

Runtime choice is capability-driven:

- Backend-internal runtime is suitable for routing-only Orchestrator work, intake refinement, dashboard assistant behavior, event processing, decision recording, and registry lookups.
- Filesystem-capable local or remote runtimes are required for Coder, Reviewer, worktree creation, git operations, builds, tests, repository inspection, and helper checks that need repository files.
- Bounded transcript read is a debug/support capability. It is not a substitute for overview/decision/lifecycle projections.

If no runtime satisfies the required capabilities, the Orchestrator records `runtime_unavailable` as a waiting reason and defers or escalates. It must not silently downgrade to the backend-internal runtime for work that requires filesystem, shell, git, or worktree authority.

## Worker identity, API, and database projection

External API identity is runtime-scoped and opaque:

- Worker detail: `GET /api/runtimes/{runtime_id}/workers/{worker_id}`.
- Cross-runtime list: `GET /api/workers`, with each item carrying `runtime_id`, `worker_id`, and display fields.

`worker-name@runtime-name` is a display label (`display_ref`) only. It is not unique enough for authority and must not be accepted as the target of mutating operations. Similarly, local Worker `worker_name`, runtime display names, raw runtime registry paths, and socket/session paths are implementation diagnostics, not API authority.

A browser-safe Worker summary can expose:

```text
worker_summary {
  runtime_id,
  worker_id,
  display_name,
  runtime_display_name,
  display_ref,
  role,
  state,
  capabilities,
  implementation: {
    kind,
    display_hint,
    worker_name?       # local Worker runtime only; diagnostic/display hint, not authority
  }
}
```

For database projection, prefer a surrogate worker record id plus a uniqueness constraint on runtime-scoped identity:

```text
workers (
  id INTEGER PRIMARY KEY,
  runtime_id TEXT NOT NULL,
  worker_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  runtime_display_name TEXT NOT NULL,
  display_ref TEXT NOT NULL,
  implementation_kind TEXT NOT NULL,
  implementation_display_hint TEXT,
  observed_at TEXT NOT NULL,
  UNIQUE(runtime_id, worker_id)
)
```

Run overviews, lifecycle events, Ticket worker links, and usage aggregates may reference the surrogate `workers.id` internally for stable joins. External APIs should continue to expose and accept only `runtime_id` + `worker_id` for Worker identity.

## Session, overview, lifecycle, and usage boundary

The Workspace backend durable projection should center on:

- orchestration events and delivery outcomes;
- routing decisions and audit records;
- worker lifecycle summaries;
- worker run overviews;
- Ticket state/relation/plan projections;
- usage aggregates.

Raw session JSONL, provider traces, verbose event streams, local sockets, and local Worker metadata files remain runtime-local source/debug logs. The backend may expose bounded debug reads later, but that surface must be explicit, purpose-limited, permissioned, size-limited, and never treated as the normal Kanban/Orchestration UI data model.

This keeps dashboard views stable across local/remote runtimes and prevents raw transcript contents from becoming hidden durable authority for why a control-plane decision happened. If a decision matters, it must be written as a decision/audit/overview record.

## Failure, blocker, and waiting reason semantics

The Orchestrator records why it did not dispatch work as carefully as why it did dispatch work. Initial reason categories:

- `dependency_blocked`: required upstream Tickets or relations are unresolved.
- `conflict_blocked`: target paths, repositories, branches, or worker assignments conflict with active work.
- `dirty_workspace`: a filesystem-capable runtime reports that the relevant checkout/worktree is dirty or unsafe. The backend-internal Orchestrator does not inspect the filesystem itself.
- `missing_requirement`: the Ticket lacks a concrete decision, acceptance criterion, permission, or scope needed to start; the Orchestrator may return it to `planning` with a reason.
- `runtime_unavailable`: no registered runtime satisfies required capabilities or capacity.
- `identity_ambiguous`: the requested Worker/runtime cannot be resolved by canonical `runtime_id` + `worker_id`.
- `forbidden_authority`: completing the request would require raw shell/filesystem/socket/session/path authority in the backend process.
- `dispatch_inconsistent`: a spawn intent retry observed inconsistent runtime acceptance evidence.

Waiting records should include ticket id, event id, reason code, human-readable summary, observed evidence, retry/unblock condition if any, and timestamp. A waiting reason may be cleared by a new Ticket event, relation change, runtime capability change, worker completion, or explicit operator action.

Returning a Ticket to `planning` requires a concrete missing-decision or missing-information reason. Risk flags, unknown implementation details, or a need for reviewer focus are not sufficient by themselves.

## Implementation sequence for future Tickets

This design suggests the following order without making any of it part of this Ticket:

1. Persist orchestration events for Kanban/Ticket state mutations, including idempotency by request id.
2. Add event delivery tools and decision/audit append tools for a backend-internal Orchestrator Worker.
3. Add runtime-scoped Worker detail APIs and backend worker projection records with surrogate ids and `UNIQUE(runtime_id, worker_id)`.
4. Add spawn intent persistence and registry dispatch stubs that preserve authority boundaries.
5. Implement local Worker runtime adaptation for Coder/Reviewer/helper intents.
6. Add remote runtime protocol only after the local typed-intent boundary is stable.

At every step, keep the invariant that durable control-plane records explain why the system acted, while runtime-specific sockets, sessions, paths, and process launch details remain adapter-local implementation details.