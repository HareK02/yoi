<!-- event: create author: tickets.sh at: 2026-05-29T20:58:44Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: review-session-pod-state-boundary at: 2026-05-29T23:04:00Z -->

## External review

Initial review found blocking issues in restore reconciliation: missing child allocations left stale runtime deny entries, and reconciliation was not enforced at the public restore boundary. The coder fixed these in commit `d2e8087`; second review approved the implementation.

Artifacts:
- `artifacts/review.md`
- `artifacts/review-r2.md`

---

<!-- event: fix author: insomnia at: 2026-05-30T00:08:00Z -->

## Parent-side validation fix

After merging the approved implementation, post-merge validation failed on `cargo test -p pod --test controller_test empty_turn_pause_rolls_back_and_snapshot_does_not_restore_input`.

The parent took over the stopped/failed handoff and fixed the adjacent turn-control regression directly on main: cancellation received immediately after the controller accepts a run was being lost before the worker reached its first stream event wait, so empty turns could hang instead of rolling back. The fix preserves idle stale-cancel cleanup at the controller boundary and makes first-event waiting cancellation-aware.

While investigating the child Pod's `context_length_exceeded` ping failure, the parent also fixed provider terminal stream errors so `Event::Error` is not only a live TUI event: terminal provider errors now fail the worker turn and persist `RunErrored` instead of allowing an empty `RunCompleted::Finished`.

---

<!-- event: close author: hare at: 2026-05-30T00:10:45Z status: closed -->

## Closed

---
id: 20260529-205844-session-pod-state-boundary
slug: session-pod-state-boundary
title: Split Pod metadata into a dedicated pod-store crate
status: closed
kind: task
priority: P2
labels: [session-store, pod-store, pod, persistence, architecture]
created_at: 2026-05-29T20:58:44Z
updated_at: 2026-05-30T00:10:45Z
assignee: null
legacy_ticket: null
---

## Background

The current persistence design intentionally has two durable surfaces:

- append-only session/segment logs, which are the authority for conversation/history state and segment lineage;
- name-keyed Pod metadata, which is the authority for Pod-name attach/restore pointers and durable spawned-child bookkeeping.

That boundary has become blurry. The `session-store` crate is named and documented primarily as session persistence, but it also owns Pod metadata types, the `PodMetadataStore` trait, validation of Pod names, and the filesystem layout `{sessions_root}/pods/{pod_name}/metadata.json`. In addition, Pod metadata currently stores `spawned_children` and `resolved_manifest_snapshot`, while session logs also store Pod scope snapshots as `LogEntry::Extension` entries. This creates a risk that session-log authority, Pod-state authority, and runtime mirrors drift or become hard to reason about.

This happened because earlier implementation work treated `session-store` as a convenient place for every durable file under the sessions root. That shape should not be extended. The chosen direction for this ticket is to split the durable surfaces into separate crates/APIs: `session-store` remains the session/segment JSONL store, and a new `pod-store` crate owns Pod metadata, Pod-name validation, and the Pod metadata filesystem layout.

## Decisions

- Introduce a dedicated `pod-store` crate for durable Pod metadata/state.
- Move Pod metadata storage from `{sessions_root}/pods/{pod_name}/metadata.json` to a top-level Pod-state root such as `{data_dir}/pods/{pod_name}/metadata.json`.
- Do not provide backward compatibility or migration for the obsolete `{sessions_root}/pods` layout. Existing old-layout Pod metadata may be ignored/lost by this change.
- Redesign the Pod metadata API where needed instead of preserving awkward `session-store`-shaped APIs.
- Keep session logs as the authority for conversation/history replay and for Pod lifecycle notifications actually shown to the model.
- Remove `pod.scope` / effective-scope snapshots from the session-log authority. Parent effective scope during restore should be derived from `pod-store` delegation state, not from a duplicate session extension.
- Keep runtime mirrors such as sockets, lock-file allocations, and `spawned_pods.json` as live runtime views, not durable authority.

Pod metadata may point at a `(SessionId, SegmentId)`, but the session log store must not own Pod metadata types or the Pod metadata filesystem layout. If sharing ID types directly causes an undesirable dependency, introduce a small shared ID module/crate or otherwise keep the dependency narrow; do not let `pod-store` pull in session replay concerns just to name a session pointer.

Observed code points:

- `crates/session-store/src/lib.rs` documents session persistence via append-only JSONL logs, but also exports `pod_metadata` types.
- `crates/session-store/src/fs_store.rs` stores segment logs under `{root}/{session_id}/{segment_id}.jsonl` and Pod metadata under `{root}/pods/{pod_name}/metadata.json` in the same `FsStore`.
- `crates/session-store/src/pod_metadata.rs` says metadata is a lightweight name-keyed pointer, but `PodMetadata` also includes `spawned_children` and `resolved_manifest_snapshot`.
- `crates/pod/src/pod.rs` writes Pod metadata from run/restore/fork/compact paths (`write_pod_metadata_active`, `write_pod_metadata_pending`) and preserves existing `spawned_children` via a read-modify-write helper.
- `crates/pod/src/spawn/registry.rs` treats durable spawned-child state as living in Pod metadata and runtime `spawned_pods.json` as a live mirror, while scope snapshots for resume live in the session log.
- `crates/tui/src/pod_list.rs` reads `{store_dir}/pods/*/metadata.json` directly in some paths rather than using only the `PodMetadataStore` trait.

## Goal

Refactor the architectural boundary between session logs, Pod metadata/state, and runtime mirrors so the storage APIs, crate boundaries, and filesystem layout match their authority boundaries, without changing intended restore/attach semantics for newly written state.

## Desired boundary

The resulting design should make these responsibilities explicit:

- Session log authority:
  - conversation history and system prompt replay;
  - segment lineage (`forked_from`, `compacted_from`);
  - request config / usage / metrics / memory extension records;
  - Pod lifecycle notifications and restore/reclaim notices only when they are appended to history as information shown to the model;
  - filesystem layout under the session log root, e.g. `{data_dir}/sessions/{session_id}/{segment_id}.jsonl` and associated trace logs.
- Pod metadata authority, owned by `pod-store`:
  - Pod-name validation and safe filesystem key rules;
  - name-keyed active `(SessionId, SegmentId)` pointer;
  - pending/new Pod state if needed before a session segment is materialized;
  - resolved manifest snapshot needed for Pod-name restore when the source profile/manifest should not be re-evaluated;
  - spawned-child registry state, because it is current parent-Pod state rather than conversation history;
  - delegated child scope records and delegation/reclaim history needed to derive parent effective scope during restore;
  - restore reconciliation state sufficient to detect children that are missing, stopped, or unreachable and to reclaim their delegated scope before continuing;
  - filesystem layout under a Pod-state root, e.g. `{data_dir}/pods/{pod_name}/metadata.json`, not below the session log root.
- Runtime mirrors:
  - sockets, lock-file allocations, and `spawned_pods.json` are live runtime views, not durable authority;
  - socket paths and callback addresses, if retained in durable metadata, must be documented as last-known runtime hints rather than proof of liveness.

## Acceptance criteria

- Create a new `pod-store` crate and move Pod metadata types/store traits/filesystem implementation into it.
- Remove `pod_metadata` exports and Pod metadata filesystem ownership from `session-store`; update `session-store` crate/module docs so it describes session/segment logs rather than Pod metadata.
- Move the durable Pod metadata layout out of `{sessions_root}/pods/{pod_name}/metadata.json` to a Pod-state root such as `{data_dir}/pods/{pod_name}/metadata.json`.
- Do not implement compatibility fallback or migration for `{sessions_root}/pods`; tests should assert the old path is not read or written as an authority.
- Redesign the Pod metadata API where useful. At minimum, avoid caller-side read-modify-write helpers that can silently drop unrelated fields; provide explicit update/merge operations or otherwise make field-preservation semantics safe and testable.
- Update construction/configuration paths so callers pass distinct roots or distinct store handles for session logs and Pod metadata; sharing the same higher-level data directory is allowed, but the session log store must not own the Pod metadata subdirectory.
- Update `pod`, `tui`, and other callers to depend on/use `pod-store` for Pod metadata instead of importing Pod metadata through `session-store` or reading metadata files directly.
- Remove direct filesystem reads of `pods/*/metadata.json` outside the `pod-store` abstraction, especially in TUI Pod list/discovery paths.
- Document the new boundary in code comments and/or crate/module docs, including why Pod metadata points to session IDs rather than being contained by the session store.
- Clarify the authority of `resolved_manifest_snapshot`: it belongs to Pod-name restore state in `pod-store`; session JSONL `SegmentStart` config/system prompt remain the authority for replaying an existing segment.
- Clarify the authority of `spawned_children`: it belongs to Pod-state/durable child-registry state in `pod-store`; child lifecycle messages shown to the model remain session JSONL history.
- Clarify delegated scope handling: delegated-scope records and delegation/reclaim history live in `pod-store`; parent effective scope during restore is derived from outstanding `pod-store` delegations. Remove the duplicate `pod.scope` session-log extension/typed restore state unless a narrower non-duplicating replacement is proven necessary.
- Add restore reconciliation behavior: when `pod-store` records a delegated child that is missing, stopped, or unreachable at restore time, reclaim the delegated scope in `pod-store`/runtime state and append a system notification to the session history before any model request observes the resumed state.
- Preserve intended durable behavior for newly written state:
  - Pod-name restore resolves active metadata from `pod-store` then restores the session log from `session-store`;
  - session restore uses session log conversation/history plus `pod-store` delegation state for Pod-scope reconciliation;
  - runtime `spawned_pods.json` remains a mirror;
  - stopped or unreachable child Pod metadata is not deleted merely because its socket is gone.
- Add focused tests for the split, including active pointer updates preserving spawned children / manifest snapshot, spawned-child updates preserving active pointer / manifest snapshot, and discovery/restore behavior when one durable surface exists without the other.
- Add or update tests that verify Pod metadata is read/written under the new Pod-state root and not under the session log root.
- Run focused validation for `session-store`, `pod-store`, `pod`, and `tui`, plus `./tickets.sh doctor` and `git diff --check`.
- Update any relevant docs or workflow notes if the persistence model changes.

## Non-goals

- Do not redesign the session-log schema unless the split proves it is necessary.
- Do not preserve backward compatibility for obsolete `{sessions_root}/pods` metadata, and do not implement a permanent fallback or migration path.
- Do not change live Pod registry lock semantics except where necessary to align with the clarified durable authority.
- Do not implement broader database storage or transactional storage in this ticket; if the boundary audit reveals a need for transactions, record it as a follow-up unless a minimal update API suffices.


---
