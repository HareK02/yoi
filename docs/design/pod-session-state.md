# Pod, session, and state authority

Yoi separates replayable history from current Pod identity because they answer different questions.

A session log answers: "what happened and what can be replayed?" Pod metadata answers: "what does this Pod name currently refer to?" Live sockets and registries answer only: "what seems reachable right now?"

## Session logs

Session JSONL is the durable replay record. It contains committed user inputs, assistant items, tool results, system/runtime events that must explain later behavior, segment boundaries, and persisted effective snapshots needed to understand a run.

The session log should be append-oriented and schema drift should be compile-visible. Compatibility shims that silently reinterpret old plural/current entries make future readers less safe.

Session logs do not own current Pod-name state. A historical session can be replayable without being the active session for a Pod name.

## Pod metadata

Pod metadata is the current-state layer keyed by Pod name. It records active/pending session pointers, resolved manifest snapshots, current delegation metadata, spawned-child visibility, and restoration information.

This avoids reconstructing current Pod state by scanning every session log. It also gives `--pod <name>`, TUI resume, `ListPods`, and `RestorePod` a single current authority.

Pod metadata should stay thin. It is not a second transcript, and it should not duplicate model conversation content.

## Live runtime hints

Sockets, process registries, and runtime files are liveness hints. They are useful for attach, status probing, and fast discovery, but they are not final proof that work completed or that a Pod's state changed durably.

A reachable pending Pod should be visible even if durable logs have not materialized yet. Missing restore labels should degrade labels and diagnostics, not hide a live attachable Pod.

## Spawned children and delegation

Parent-visible children are sourced from Pod metadata, not from a transient runtime mirror. Restoring a parent should reconstruct reachable children where possible and keep stopped-but-restorable children visible when metadata supports it.

Delegated write scope is a capability loan. Stopping, shutting down, or pruning a child must reclaim the parent's effective write permissions while preserving explicit base denies.

## Peer Pods

Peer visibility is also Pod metadata, but it is distinct from spawned-child delegation. A TUI user can run `:peer <pod-name>` while attached to an idle Pod to register a reciprocal peer handshake with another existing Pod.

A peer relationship only makes the Pods mutually visible through `ListPods` with visibility source `peer`. It does not grant filesystem scope, create a child output cursor, make either Pod the other's parent, or imply child completion notifications. Peer messages use `SendToPeerPod`, which delivers a labeled notification into the target Pod's normal durable notification/history path.

## Notifications are not authority

Pod completion notifications are UX hints. Before treating delegated work as complete, inspect queryable evidence: child output, session/log state, worktree status, diffs, and validation output.

This is why orchestration code should expose state-aware operations such as `ListPods` and `RestorePod`, rather than letting a background alert decide workflow state by itself.
