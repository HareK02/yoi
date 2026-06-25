# Worker, session, and state authority

Yoi separates replayable history from current Worker identity because they answer different questions.

A session log answers: "what happened and what can be replayed?" Worker metadata answers: "what does this Worker name currently refer to?" Live sockets and registries answer only: "what seems reachable right now?"

## Session logs

Session JSONL is the durable replay record. It contains committed user inputs, assistant items, tool results, system/runtime events that must explain later behavior, segment boundaries, and persisted effective snapshots needed to understand a run.

The session log should be append-oriented and schema drift should be compile-visible. Compatibility shims that silently reinterpret old plural/current entries make future readers less safe.

Session logs do not own current Worker-name state. A historical session can be replayable without being the active session for a Worker name.

## Worker metadata

Worker metadata is the current-state layer keyed by Worker name. It records active/pending session pointers, resolved manifest snapshots, current delegation metadata, spawned-child visibility, and restoration information.

This avoids reconstructing current Worker state by scanning every session log. It also gives `--worker <name>`, TUI resume, `ListWorkers`, and `RestoreWorker` a single current authority.

Worker metadata should stay thin. It is not a second transcript, and it should not duplicate model conversation content.

## Live runtime hints

Sockets, process registries, and runtime files are liveness hints. They are useful for attach, status probing, and fast discovery, but they are not final proof that work completed or that a Worker's state changed durably.

A reachable pending Worker should be visible even if durable logs have not materialized yet. Missing restore labels should degrade labels and diagnostics, not hide a live attachable Worker.

## Spawned children and delegation

Parent-visible children are sourced from Worker metadata, not from a transient runtime mirror. Restoring a parent should reconstruct reachable children where possible and keep stopped-but-restorable children visible when metadata supports it.

Delegated write scope is a capability loan. Stopping, shutting down, or pruning a child must reclaim the parent's effective write permissions while preserving explicit base denies.

## Peer Workers

Peer visibility is also Worker metadata, but it is distinct from spawned-child delegation. A TUI user can run `:peer <worker-name>` while attached to an idle Worker to register reciprocal peer metadata with another existing Worker. This is a metadata-level registration, not live target-controller consent.

A peer relationship only makes the Workers mutually visible through `ListWorkers` with visibility source `peer`. It does not grant filesystem scope, create a child output cursor, make either Worker the other's parent, or imply child completion notifications. Peer messages use `SendToPeerPod`, which delivers a labeled notification into the target Worker's normal durable notification/history path. `SendToPeerPod` requires the peer to be live and fails clearly for non-live peers rather than auto-restoring them.

## Notifications are not authority

Worker completion notifications are UX hints. Before treating delegated work as complete, inspect queryable evidence: child output, session/log state, worktree status, diffs, and validation output.

This is why orchestration code should expose state-aware operations such as `ListWorkers` and `RestoreWorker`, rather than letting a background alert decide workflow state by itself.
