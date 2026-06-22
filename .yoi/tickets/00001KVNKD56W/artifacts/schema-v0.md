# Workspace DB canonical schema v0 design

## Purpose

This document defines the first concrete Workspace control-plane schema target. It is precise enough that implementation work can create migrations and API read/write surfaces without inventing meanings ad hoc.

The important corrections in this version are:

- **Ticket thread/events remain the execution history authority**.
- A separate top-level `Run` entity is not part of v0.
- Separate `validation_results` / CI tables are not part of v0.
- Worker is not a DB-canonical entity in v0. Worker state is obtained from runtime inspection / Host protocol as a live view.
- Ticket-to-Worker management is represented by typed WorkerRef snapshots on Ticket events and Ticket-Worker association records.
- v0 does **not** use generic JSON payload/metadata columns. If a value matters, give it a typed column or a small relation table. If it is large evidence, store it as an Artifact.

## Schema categories

1. **Current-state records**: long-lived records with stable ids and current snapshots, such as Ticket, Objective, Repository, Artifact.
2. **Event logs**: append-oriented records attached to current-state records, primarily `ticket_events` and `audit_events`.
3. **Relationship records**: explicit links such as Ticket-to-WorkerRef, Ticket-to-Repository target, Objective-to-Ticket.
4. **Snapshot references**: typed authorship / worker / host references embedded in event or relation records. These are not full entities in v0.
5. **Live views**: API results produced by inspecting local runtime or future Host protocol state. Host/Worker lists are live views in v0, not canonical DB tables.

All main tables include `workspace_id`. v0 is SQLite-first, but table shapes should not prevent later Postgres/multi-workspace hosting.

## Design rules

- Ticket and Objective belong to Workspace, not to Repository.
- Repository is a Workspace-connected source/storage. Git Repository is one provider, not the definition of Repository.
- Ticket target selectors are mutable intent/scope. Evidence artifacts may record the concrete repository revision they were produced from with typed source fields.
- Ticket thread is the human-readable and structured execution/audit history for work on that Ticket.
- Ticket current state is a snapshot derived/maintained from structured state transition events.
- Worker is a logical agent/session participating in work, but Worker registry/persistence is out of v0 DB scope.
- Host is an execution environment or observed placement. In v0, Host/Worker information is returned as a live view from local runtime inspection or future Host protocol, not stored as canonical DB records.
- Ticket-associated Worker management uses WorkerRef fields and `ticket_worker_links` snapshots. This lets the Ticket be managed without making the Worker itself DB-canonical.
- Orchestrator should be able to operate from DB/API records only: Ticket, TicketEvents, TicketWorkerLinks, live Host/Worker views, Artifact, and review/evidence summaries.
- Raw fs/Bash/Git authority belongs to Host/Worker execution, not to Orchestrator.
- Memory/Knowledge are intentionally out of v0 canonical schema. They are deferred until Workspace storage migration for Memory.
- Event authorship is mandatory, but a full Actor table is not required in v0.
- Generic JSON columns are intentionally excluded in v0. Do not add `metadata_json`, `payload_json`, `diagnostics_json`, or similar catch-all fields.

## Common columns and conventions

### IDs

Use opaque string ids allocated by the control plane for DB-canonical records.

Recommended prefixes are implementation detail, but the type must be obvious from column names:

- `workspace_id`
- `ticket_id`
- `event_id`
- `objective_id`
- `repository_id`
- `target_id`
- `artifact_id`
- `audit_event_id`

Worker and Host references use `*_ref_kind` / `*_ref_key` in v0 because they are not canonical DB entities.

### Timestamps

Store UTC timestamps as RFC3339 strings in SQLite v0.

Common names:

- `created_at`
- `updated_at`
- `observed_at`
- `started_at`
- `finished_at`
- `closed_at`
- `last_seen_at`

### No catch-all payload columns

v0 avoids generic JSON/text payload columns because they make the schema ambiguous and move authority into untyped blobs.

Rules:

- Fields used for lifecycle transitions, permissions, joins, filtering, or orchestration decisions must be typed columns or relation tables.
- Event kinds may have nullable typed columns such as `subject_kind`, `subject_id`, `previous_state`, `new_state`, `status`, `activity_id`, or `artifact_id`.
- Repository capabilities are derived from `repositories.kind` / `repositories.provider` and backend configuration in v0; do not add a separate capability table until provider-specific overrides are actually needed.
- Paths use relation tables such as `ticket_target_paths`.
- Diagnostics that matter should be Ticket events or Artifacts.
- Large logs, diffs, transcripts, prompts, raw tool outputs, and file contents must not be embedded in records. Store them in an artifact file/blob store and link through Artifact URI records.
- Secrets are never stored in this schema. Secret references, if needed, use typed reference columns such as `auth_ref_kind` and `auth_ref_key`.

## Authorship fields v0

Authorship is an embedded typed snapshot, not a full table in v0.

Use the following columns on event/request/audit records that need authorship:

```text
author_kind text not null
author_key text not null
author_display text not null
author_source_kind text null
author_source_key text null
```

`author_kind` allowed values:

- `human`
- `agent`
- `system`
- `integration`
- `unknown`

`author_key` is stable within its source namespace, for example:

- `local-user`
- `agent:orchestrator`
- `worker:<worker_ref_key>`
- `system:yoi-control-plane`
- `integration:ci:<provider>`

`author_display` is a display snapshot at event creation time. It must be sufficient for historical display even if a future Actor/User record changes name.

`author_source_kind` and `author_source_key` can point to bounded source context such as `worker`, `profile`, `external_account`, or `provider`. They must not hold secrets.

A future `actors` table may be added for auth, assignment, team membership, and permissions. v0 must not require it. If it is added later, historical events still keep their authorship snapshot and may optionally link to `actor_id`.

## WorkerRef and HostRef v0

Worker and Host are runtime concepts in v0. They are referenced by typed snapshots instead of DB foreign keys.

Use WorkerRef fields where a Ticket event, Ticket association, artifact, or check report needs to identify a Worker:

```text
worker_ref_kind text null -- local_pod | remote_worker | hosted_worker | external | unknown
worker_ref_key text null
worker_display text null
```

Examples:

- `worker_ref_kind = local_pod`, `worker_ref_key = coder-sidebar`, `worker_display = Coder sidebar`
- `worker_ref_kind = hosted_worker`, `worker_ref_key = worker_...`, `worker_display = Hosted coder`

Use HostRef fields only when observed placement matters:

```text
host_ref_kind text null -- local | self_hosted | cloud | external | unknown
host_ref_key text null
host_display text null
```

HostRef is not ownership. It means “this Worker or event was observed on this execution environment at this time”.

Future work may add canonical `workers`, `hosts`, `worker_archive`, and `host_connections` tables when Worker lifecycle, persistence, and archive requirements are concrete. v0 deliberately does not create those tables.

## Execution model without a Run entity

v0 does not create a separate `runs` table.

A concrete execution attempt is represented by:

- a `ticket_event` such as `execution_requested`, `worker_assigned`, `worker_status`, `implementation_report`, `review`, `check_report`, `artifact_link`, or `state_transition`;
- optional `activity_id` on related `ticket_events` to group a burst of execution activity;
- `ticket_worker_links` records showing which WorkerRefs are associated with the Ticket and in what role/status;
- `artifacts` linked to `ticket_id`, `event_id`, optional WorkerRef fields, and optional typed repository source revision fields.

`activity_id` is a correlation key, not an authority entity. It can be generated when a user/Orchestrator accepts an execution request, but the Ticket thread remains the authority.

This avoids duplicating Ticket events and Run records while preserving machine-readable execution state.

## Live Host/Worker API view

v0 API may expose Host and Worker lists, but they are live views, not DB tables.

Examples:

- `GET /api/hosts` may inspect the backend-local machine and return one synthetic local Host.
- `GET /api/workers` may scan current local Pod metadata and sockets and return Worker summaries.
- Future Host protocol can provide the same API shape from heartbeat/connection state.

These API responses must not imply DB persistence. If a Worker disappears from runtime inspection, it can disappear from the live view. Durable history belongs to Ticket events, TicketWorkerLinks, and Artifacts.

## Tables

### `workspaces`

```text
workspace_id text primary key
display_name text not null
state text not null -- active | archived
created_at text not null
updated_at text not null
```

### `tickets`

Current Ticket state and body snapshot.

```text
workspace_id text not null
ticket_id text primary key
title text not null
state text not null -- planning | ready | queued | inprogress | done | closed
priority text null
assignee_kind text null
assignee_key text null
assignee_display text null
body_md text not null
created_at text not null
updated_at text not null
closed_at text null
resolution_event_id text null
```

Notes:

- `tickets` stores the current read model.
- Historical changes belong to `ticket_events`.
- Ticket state transitions must be represented by structured `ticket_events`.
- Assignee is a snapshot, not a foreign key to `actors` in v0.

### `ticket_events`

Append-oriented Ticket thread/event log. This is also the execution history authority for work on a Ticket.

```text
workspace_id text not null
event_id text primary key
ticket_id text not null
event_seq integer not null
kind text not null
activity_id text null
author_kind text not null
author_key text not null
author_display text not null
author_source_kind text null
author_source_key text null
created_at text not null
body_md text null
subject_kind text null -- ticket | worker | artifact | check | repository | objective | system
subject_id text null
previous_state text null
new_state text null
status text null
artifact_id text null
worker_ref_kind text null
worker_ref_key text null
worker_display text null
host_ref_kind text null
host_ref_key text null
host_display text null
repository_id text null
caused_by_event_id text null
```

`kind` allowed values in v0:

- `comment`
- `plan`
- `decision`
- `review`
- `implementation_report`
- `state_transition`
- `close`
- `execution_requested`
- `worker_assigned`
- `worker_status`
- `check_report`
- `artifact_link`
- `system_note`

Constraints:

- unique `(ticket_id, event_seq)`.
- events are append-only except administrative repair migrations.
- state transitions and close events must include `previous_state` and `new_state` where applicable.
- execution events should use typed columns such as `activity_id`, WorkerRef fields, `artifact_id`, and `repository_id` instead of opaque payloads.

### `ticket_relations`

```text
workspace_id text not null
source_ticket_id text not null
target_ticket_id text not null
kind text not null -- depends_on | blocks | related | supersedes | duplicate_of
created_at text not null
author_kind text not null
author_key text not null
author_display text not null
author_source_kind text null
author_source_key text null
note text null
primary key (source_ticket_id, target_ticket_id, kind)
```

### `objectives`

```text
workspace_id text not null
objective_id text primary key
title text not null
state text not null -- active | paused | done | closed | archived
body_md text not null
created_at text not null
updated_at text not null
```

### `objective_ticket_links`

```text
workspace_id text not null
objective_id text not null
ticket_id text not null
kind text not null -- tracks | related | milestone | blocker
created_at text not null
primary key (objective_id, ticket_id, kind)
```

### `repositories`

Workspace-connected source/storage. Git is one provider.

```text
workspace_id text not null
repository_id text primary key
name text not null
kind text not null -- git | local | object_store | artifact_store | custom
provider text null -- git, local_fs, s3, etc.
uri text not null
default_ref text null
auth_ref_kind text null
auth_ref_key text null
created_at text not null
updated_at text not null
```

Notes:

- `uri` is identity/config data. It may be redacted in API responses.
- `auth_ref_kind` / `auth_ref_key` contain secret references only, never secret values.
- v0 does not store per-Repository capability rows. Capabilities are derived from `kind`, `provider`, and backend configuration. Add explicit capability/override records later only if a real provider needs per-Repository variance.

### `ticket_targets`

Ticket scope/intent against one or more Repositories.

```text
workspace_id text not null
ticket_id text not null
target_id text not null
repository_id text not null
role text not null -- primary | related | reference | check | output
intent text not null -- read | change | check | output
ref_selector text null
created_at text not null
updated_at text not null
primary key (ticket_id, target_id)
```

### `ticket_target_paths`

```text
workspace_id text not null
ticket_id text not null
target_id text not null
path text not null
primary key (ticket_id, target_id, path)
```

### `ticket_worker_links`

Current relationship between Ticket and a WorkerRef.

```text
workspace_id text not null
ticket_id text not null
worker_ref_kind text not null
worker_ref_key text not null
worker_display text null
role text not null -- companion | intake | orchestrator | coder | reviewer | validator | custom
status text not null -- requested | assigned | active | blocked | completed | released | failed | cancelled
activity_id text null
assigned_at text null
released_at text null
last_event_id text null
primary key (ticket_id, worker_ref_kind, worker_ref_key, role)
```

Notes:

- This is the main DB management relation for Ticket-associated Workers.
- It is not a Worker registry.
- Ticket thread events should record assignment/release/status changes.

### `artifacts`

Evidence/output linked to Ticket, Objective, event, WorkerRef, or Repository source revision.

Artifact content is not stored inline in the DB. Every Artifact points to a URI. The URI may be served by the Workspace backend's artifact/static-file service, a blob store, or an external system.

```text
workspace_id text not null
artifact_id text primary key
kind text not null -- diff | patch | log | report | check_report | review | file | external_link | summary
uri text not null
media_type text null
sha256 text null
size_bytes integer null
summary text null
created_at text not null
created_by_kind text not null
created_by_key text not null
created_by_display text not null
created_by_source_kind text null
created_by_source_key text null
ticket_id text null
objective_id text null
event_id text null
worker_ref_kind text null
worker_ref_key text null
worker_display text null
repository_id text null
source_kind text null -- git_commit | file_snapshot | object_version | custom
source_revision text null -- commit hash, snapshot id, or object version id
```

Rules:

- `uri` is mandatory.
- DB rows store metadata and summary only, never artifact body content.
- `source_kind` / `source_revision` are optional typed source fields for artifacts produced against a concrete repository revision. They do not represent branch/ref selectors; mutable selectors remain on `ticket_targets.ref_selector` or in the related Ticket event.
- Workspace-owned artifact content should use a stable internal URI scheme or backend-served URL, for example `artifact://<workspace_id>/<artifact_id>` or `/api/artifacts/<artifact_id>/content`.
- External artifacts may use redacted `https://...` or provider-specific URIs when policy allows.
- API list/detail responses return artifact metadata and URI by default. Fetching content is a separate artifact-content operation with bounds and permission checks.

## CI / actions-like checks are future work

v0 does not add `validation_results`, `ci_results`, or action tables.

For now, local checks, CI summaries, and check evidence are represented by:

- `ticket_events.kind = check_report` or `artifact_link`;
- Artifacts such as logs, check reports, or external CI URLs;
- Ticket state transitions or review events that reference those artifacts.

If first-class CI status is needed, design it as a separate actions-like subsystem rather than a generic validation table inside the core Ticket schema. That future subsystem should model workflow/check names, jobs, steps, attempts, statuses, logs, annotations, external provider ids, retention, and rerun semantics explicitly.

### `audit_events`

Control-plane operation audit trail.

```text
workspace_id text not null
audit_event_id text primary key
created_at text not null
actor_kind text not null
actor_key text not null
actor_display text not null
actor_source_kind text null
actor_source_key text null
action text not null
target_kind text not null
target_id text null
outcome text not null -- allowed | denied | succeeded | failed
request_id text null
summary text null
```

Audit events record the control-plane action and outcome. They should not duplicate full Ticket event payloads unless needed for audit.

## Read surfaces for Orchestrator without fs/Bash

The DB/API must let an Orchestrator read:

- Ticket current state, thread events, relations, targets, and TicketWorkerLinks.
- Objective body and linked Tickets.
- Repository summaries, Ticket target selectors, and Artifact source revision fields.
- Live Host/Worker views from runtime inspection or future Host protocol.
- Artifact summaries and selected artifact contents through bounded artifact APIs.
- Check/CI summaries as TicketEvents and Artifacts.
- Review evidence as TicketEvents/Artifacts.

## Write surfaces for Orchestrator without fs/Bash

The DB/API must let an Orchestrator create:

- Ticket comments/decisions/state transition requests.
- Ticket execution request events with target selectors and optional `activity_id`.
- TicketWorkerLink assignment/release/status changes.
- Review/check request events.
- Artifact links for logs, reports, diffs, CI/external check URLs, and review evidence.
- Close/done decisions that reference evidence artifacts and structured Ticket events.

The Orchestrator must not need raw repository filesystem reads, shell execution, or direct Git merge authority to perform control-plane routing.

## Migration stance

v0 implementation should support three modes conceptually:

1. `filesystem_read_through`: current `.yoi/tickets` and `.yoi/objectives` remain authority; DB holds runtime/projection tables.
2. `imported_projection`: filesystem records are imported into DB read models, but filesystem remains the write authority.
3. `db_authority`: Ticket/Objective write path moves to DB; filesystem export becomes compatibility/export snapshot.

This Ticket designs the schema target and can implement non-breaking migrations, but it does not require switching active authority to DB.

## Minimal implementation guidance

If implementation is included in this Ticket, prefer a small non-breaking migration:

- Keep Host/Worker API as live runtime views in v0.
- Add explicit schema versioning.
- Add tables that are safe to create empty: `repositories`, `ticket_targets`, `ticket_target_paths`, `ticket_worker_links`, `artifacts`, `audit_events`.
- Keep existing filesystem read APIs working.
- Do not create a full `actors` table in v0.
- Do not create `hosts` / `workers` canonical tables in v0.
- Do not create a separate `runs` table in v0; use structured Ticket events and TicketWorkerLink relationships.

## Implementation alignment notes

The `yoi-workspace-server` SQLite bootstrap migration implements this v0 schema as schema version 2. Fresh databases create the typed tables listed above and deliberately do not create canonical `runs`, `hosts`, `workers`, `actors`, or check/validation result tables. Host and Worker HTTP read APIs remain live runtime views backed by local inspection, not DB tables.

For databases created by the earlier workspace-server bootstrap, migration version 2 preserves old `repositories`, `runs`, `artifacts`, `ticket_projections`, and `objective_projections` data by renaming those tables to `legacy_repositories`, `legacy_runs`, `legacy_artifacts`, `legacy_ticket_projections`, and `legacy_objective_projections`, then creating the v0 typed tables. The legacy names are compatibility preservation only and are not canonical schema tables or active write authority.
