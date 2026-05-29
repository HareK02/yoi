---
id: 20260529-205844-session-pod-state-boundary
slug: session-pod-state-boundary
title: Split session log storage from Pod metadata storage
status: open
kind: task
priority: P2
labels: [session-store, pod, persistence, architecture]
created_at: 2026-05-29T20:58:44Z
updated_at: 2026-05-29T21:30:26Z
assignee: null
legacy_ticket: null
---

## Background

The current persistence design intentionally has two durable surfaces:

- append-only session/segment logs, which are the authority for conversation/history state and segment lineage;
- name-keyed Pod metadata, which is supposed to be a thin pointer layer for Pod-name attach/restore and spawned-child bookkeeping.

That boundary has become blurry. The `session-store` crate is named and documented primarily as session persistence, but it also owns Pod metadata types, the `PodMetadataStore` trait, validation of Pod names, and the filesystem layout `{sessions_root}/pods/{pod_name}/metadata.json`. In addition, Pod metadata currently stores `spawned_children` and `resolved_manifest_snapshot`, while session logs also store Pod scope snapshots as `LogEntry::Extension` entries. This creates a risk that session-log authority, Pod-state authority, and runtime mirrors drift or become hard to reason about.

The chosen direction for this ticket is to split the durable surfaces instead of documenting the current shape as acceptable. Pod metadata is not a child resource of the session log store: it should live under a Pod-state root such as `{data_dir}/pods/{pod_name}/metadata.json`, while segment logs remain under `{data_dir}/sessions/{session_id}/{segment_id}.jsonl`. Pod metadata may point at a `(SessionId, SegmentId)`, but the session log store must not own Pod metadata types or the Pod metadata filesystem layout.

Observed code points:

- `crates/session-store/src/lib.rs` documents session persistence via append-only JSONL logs, but also exports `pod_metadata` types.
- `crates/session-store/src/fs_store.rs` stores segment logs under `{root}/{session_id}/{segment_id}.jsonl` and Pod metadata under `{root}/pods/{pod_name}/metadata.json` in the same `FsStore`.
- `crates/session-store/src/pod_metadata.rs` says metadata is a lightweight name-keyed pointer, but `PodMetadata` also includes `spawned_children` and `resolved_manifest_snapshot`.
- `crates/pod/src/pod.rs` writes Pod metadata from run/restore/fork/compact paths (`write_pod_metadata_active`, `write_pod_metadata_pending`) and preserves existing `spawned_children` via a read-modify-write helper.
- `crates/pod/src/spawn/registry.rs` treats durable spawned-child state as living in Pod metadata and runtime `spawned_pods.json` as a live mirror, while scope snapshots for resume live in the session log.
- `crates/tui/src/pod_list.rs` reads `{store_dir}/pods/*/metadata.json` directly in some paths rather than using only the `PodMetadataStore` trait.

## Goal

Refactor the architectural boundary between session logs, Pod metadata/state, and runtime mirrors so the storage APIs and filesystem layout match their authority boundaries, without changing user-visible restore/attach semantics.

## Desired boundary

The resulting design should make these responsibilities explicit:

- Session log authority:
  - conversation history and system prompt replay;
  - segment lineage (`forked_from`, `compacted_from`);
  - request config / usage / metrics / memory extension records;
  - Pod runtime scope snapshots required to restore the same session without silently reclaiming delegated writes;
  - filesystem layout under the session log root, e.g. `{data_dir}/sessions/{session_id}/{segment_id}.jsonl` and associated trace logs.
- Pod metadata authority:
  - name-keyed active `(SessionId, SegmentId)` pointer;
  - resolved manifest snapshot needed for Pod-name restore when the source profile/manifest should not be re-evaluated;
  - spawned-child registry state, if retained here, with a documented reason why it is Pod state rather than session state;
  - filesystem layout under a Pod-state root, e.g. `{data_dir}/pods/{pod_name}/metadata.json`, not below the session log root.
- Runtime mirrors:
  - sockets, lock-file allocations, and `spawned_pods.json` are live runtime views, not durable authority.

## Acceptance criteria

- Audit every public type/function in `session-store` related to Pod metadata and classify whether it is genuinely session-log responsibility or Pod-state responsibility.
- Move/split Pod metadata APIs out of the session log API surface so session log APIs do not expose Pod metadata concepts unnecessarily.
- Move the durable Pod metadata layout out of `{sessions_root}/pods/{pod_name}/metadata.json` to a Pod-state root such as `{data_dir}/pods/{pod_name}/metadata.json`.
- Update construction/configuration paths so callers pass distinct roots or distinct store handles for session logs and Pod metadata; sharing the same higher-level data directory is allowed, but the session log store must not own the Pod metadata subdirectory.
- Document the new boundary in code comments and/or crate/module docs, including why Pod metadata points to session IDs rather than being contained by the session store.
- Remove direct filesystem reads of `pods/*/metadata.json` outside the Pod metadata store abstraction, especially in TUI Pod list/discovery paths.
- Clarify the authority of `resolved_manifest_snapshot`: it belongs to Pod-name restore state unless a different Pod-state record is deliberately introduced; ensure restore paths follow the documented authority.
- Clarify the authority of `spawned_children`: it belongs to Pod-state/durable child-registry state unless deliberately moved; ensure restore/prune/reclaim behavior follows the documented authority.
- Ensure read-modify-write preservation of unrelated Pod metadata fields does not silently lose data when active pointer updates and spawned-child updates occur near each other; either make the update semantics explicit or add a safer merge/update API.
- Preserve the current durable behavior unless deliberately changed:
  - Pod-name restore resolves active metadata then restores the session log;
  - session restore uses session log state and scope snapshots;
  - runtime `spawned_pods.json` remains a mirror;
  - stopped or unreachable child Pod metadata is not deleted merely because its socket is gone.
- Decide and document the migration stance for existing `{sessions_root}/pods` data. If migration is provided, it must be one-shot and must not keep the old path as a long-term fallback authority.
- Add focused tests for the chosen split, including active pointer updates preserving spawned children / manifest snapshot, spawned-child updates preserving active pointer / manifest snapshot, and discovery/restore behavior when one durable surface exists without the other.
- Add or update tests that verify Pod metadata is read/written under the new Pod-state root and not under the session log root.
- Update any relevant docs or workflow notes if the persistence model changes.

## Non-goals

- Do not redesign the session-log schema unless the audit proves it is necessary.
- Do not keep backward compatibility for obsolete persistence layouts as a permanent fallback; a one-shot migration may be implemented only if explicitly chosen and documented.
- Do not change live Pod registry lock semantics except where necessary to align with the clarified durable authority.
- Do not implement broader database storage or transactional storage in this ticket; if the boundary audit reveals a need for transactions, record it as a follow-up unless a minimal update API suffices.
