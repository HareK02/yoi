# Worker aggregate, Session, and run authority

A Runtime-managed Worker is the canonical durable aggregate. One Worker owns exactly one Session for its lifetime; compaction and forks create Segments inside that Session rather than replacing the Session identity.

This identity rule separates durable conversation history from execution attempts:

- **Worker ID** identifies the Runtime catalog aggregate and remains stable across stop/restore.
- **Session ID** identifies that Worker's sole replayable history and remains stable across compaction.
- **Segment ID** identifies a branch or compacted history projection inside the Session.
- **run identity** identifies one process/controller execution attempt. Fresh spawn uses a unique run ID; Restore uses the durable lifecycle operation ID so reconciliation addresses the same attempt rather than allocating another controller.

## Canonical Runtime layout

Filesystem Runtime stores materialize one aggregate under `workers/<worker_id>/`:

```text
workers/<worker_id>/
  worker.json
  metadata.json
  session/
    session.json
    segments/
      <segment_id>.jsonl
      <segment_id>.trace.jsonl
  runs/
    <spawn-run-id>/
      worker.sock
      worker.out.log
      worker.err.log
      artifacts/
      spawned/
    restore-<lifecycle-operation-id>/
      worker.sock
      worker.out.log
      worker.err.log
      artifacts/
      spawned/
```

`worker.json` is the atomic Runtime catalog authority: Worker identity and Workspace attribution, the settled lifecycle status, the create/restore request and execution binding, plus any pending Restore or Stop reconciliation operation. `metadata.json` is the current Worker projection used to restore active/pending Segment pointers, resolved manifest state, delegation metadata, and child/peer visibility. It is not a second transcript.

`session/session.json` fixes the single Session ID for the aggregate. The Worker-specific Session store rejects attempts to address another Session ID. Session JSONL is the append-oriented replay authority for committed user inputs, assistant items, tool results, system/runtime events, Segment lineage, and effective snapshots required to explain later behavior.

The normal execution path resolves all three stores from the trusted `WorkerRef`. It does not fall back to process-global Session or Worker-metadata roots. Those roots are legacy migration inputs only.

## Segment lifecycle

A new Worker materializes its Session when the initial Segment is created. The Session ID then remains stable.

Compaction writes a new Segment in the same Session with `compacted_from` lineage. Forking likewise writes a sibling Segment in the same Session with `forked_from` lineage. Allocation, UI, and event surfaces therefore report the new **Segment ID**, never a "new Session". A different Session requires a different Worker aggregate.

This keeps existing Worker IDs, Session IDs, Segment references, observation entry references, and UI routes meaningful across compaction and restore.

## Run lifecycle

A lifecycle transition has a stable operation ID distinct from settled Worker status. Runtime commits a pending operation into `worker.json` before invoking a side-effecting Backend call:

- fresh spawn uses a unique run ID and commits the connected aggregate only after Backend startup succeeds;
- Restore records `ReconciliationRequired::Restore(operation_id, mode, last_settled_status, restore request)` after preflight and before controller startup;
- Stop records `ReconciliationRequired::Stop(operation_id, last_settled_status, execution evidence)` before requesting controller shutdown.

Restore run directories are named `restore-<operation_id>` and use `create_new` semantics. Retry and startup reconciliation reuse that operation ID. An in-process Backend returns an already registered controller for the same operation; after Backend restart, the existing operation-scoped directory prevents the same operation from allocating a second run generation. A different Restore operation receives a different operation ID and directory only after the previous operation has reached a proved terminal outcome.

Startup reconciles pending Stop operations before considering automatic Restore. A proved Stop commits `Stopped` atomically and is therefore never selected as an active restore candidate. Pending or uncertain operations remain orthogonal to the last settled status and block conflicting input, lifecycle, deletion, and retention operations.

Restore candidates keep protocol events and observation registration isolated until the Active aggregate commit succeeds. After that commit, Runtime activates the Backend observation registration and flushes buffered protocol events in order. A rollback discards the candidate context, so external observers cannot see a live candidate beside a settled `Stopped` catalog record.

Stopping a Worker waits for controller shutdown completion and removes `worker.sock`. Run directories and diagnostic files remain evidence. How old runs are retained, archived, or purged is a separate policy; aggregate creation and migration do not invent that disposition.

Live sockets and provider sessions are execution hints, not durable identity. Restore reconstructs Workspace client attribution, observation registration, and Workdir/provider bindings from Runtime/Backend authority while keeping the Worker and Session identities unchanged.

## Legacy migration

Startup migration recognizes the versioned `worker-aggregate-v1` format and treats legacy process-global metadata/Session directories as read-only sources.

The migration is serialized by an OS file lock and writes a versioned manifest with per-Worker checkpoints and diagnostics. For each unambiguous catalog Worker it:

1. validates Worker metadata identity and the referenced Session;
2. parses Segment and trace JSONL, tolerating only the existing crash-truncated final-line rule;
3. stages `session.json` plus all Segment files under the target Worker aggregate;
4. fsyncs files and directories, atomically renames the staged Session directory, and fsyncs its parent;
5. atomically writes and fsyncs `metadata.json`;
6. atomically updates the migration checkpoint.

Reruns validate exact Session identity, the complete source/target filename set, and file bytes before accepting an existing target. Mixed old/new stores and a crash between Session rename, metadata copy, and checkpoint update therefore converge without replacing divergent data.

Migration fails closed on target collisions, corrupt complete JSONL records, or ambiguous ownership. If multiple legacy metadata sources reference one Session—including metadata with no catalog Worker—the Session is not assigned to either aggregate. The manifest records the shared reference. Legacy metadata or Session directories with no catalog-backed owner are also recorded as orphans and left untouched; they do not become normal authority and are not silently deleted.

Legacy sources remain available for audit and recovery after a successful copy. Archive/retention disposition and Orchestrator-driven Worker removal are intentionally outside this migration contract.

## Child, peer, and notification state

Parent-visible children and peer registrations are current Worker metadata, distinct from Session history and run liveness. Restoring a parent reconstructs reachable children where possible and retains stopped-but-restorable visibility when metadata supports it. Delegated write scope is a capability loan; stopping or pruning a child must reclaim the parent's effective permissions.

Peer registration does not grant filesystem authority, imply parent ownership, or make notifications completion proof. Notifications remain UX hints committed through the normal Worker history path. Completion decisions must reread durable Ticket, repository, review, and test evidence rather than relying on a socket event or final assistant message.
