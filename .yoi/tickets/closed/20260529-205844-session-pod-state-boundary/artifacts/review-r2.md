# Review R2: session-pod-state-boundary

Verdict: approve

## Conceptual summary

Commit `d2e8087` addresses the two prior blocking issues without reintroducing session-log scope authority. The restore path now reconciles missing/unreachable delegated children inside `Pod::restore_from_manifest` before returning a usable `Pod`, and `pod_registry::reclaim_delegated_scope` now removes the parent's delegated deny layer even when the child allocation is already absent.

## Findings

No blocking issues found in the reviewed delta.

The specific R1 blockers are resolved:

- `crates/pod-registry/src/mutate.rs:181-224` now removes matching `parent_alloc.scope_deny` entries unconditionally for delegated write rules, before optionally removing/reparenting an existing child allocation. This covers the missing-child allocation case and remains idempotent for absent deny entries.
- `crates/pod-registry/src/mutate.rs:524-551` adds direct coverage for reclaiming parent deny when the child allocation is missing.
- `crates/pod/src/pod.rs:4056` calls `pod.reconcile_restored_delegations().await?` from `Pod::restore_from_manifest` before returning the restored `Pod`; `restore_from_pod_metadata` still delegates through `restore_from_manifest`, so name-based restore gets the same enforcement.
- `crates/pod/src/pod.rs:4061-4108` performs reachability checks, reclaims runtime lock state, updates in-memory scope, moves reclaimed children into `pod-store`, and queues a notification via `push_notify` before the restored Pod can be used for a model request.
- Grep review did not find reintroduced `pod.scope` / effective-scope session authority. Remaining `LogEntry::Extension` uses are metrics/memory or generic replay handling, not Pod scope snapshots.

## Non-blocking notes

- `PodController::spawn` still has a secondary `SpawnedPodRegistry::load_from_pod_state_with_reclaim` reconciliation/notification path. With the constructor-level reconciliation, restored Pods should normally arrive there already cleaned up; the remaining path is still useful for registry construction and non-restore cases, but future cleanup could avoid duplicate conceptual ownership.
- As noted in R1, `pod-store::PodMetadataStore::update_by_name` remains read-modify-write internally. That is acceptable for this ticket's filesystem backend and covered field-preservation semantics, but it is not a transactional concurrency primitive.

## Validation run

Inspected the `d2e8087` delta and the current relevant files:

- `crates/pod-registry/src/mutate.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/tests/pod_comm_tools_test.rs`
- `crates/pod/tests/restore_test.rs`

Commands run from `/home/hare/Projects/insomnia/.worktree/session-pod-state-boundary`:

```text
cargo test -p pod-registry reclaim_delegated_scope
cargo test -p pod --test pod_comm_tools_test load_from_pod_state_reclaims_missing_child_scope_and_records_history
cargo test -p pod --test restore_test
git diff --check HEAD~3..HEAD
```

All commands passed.
