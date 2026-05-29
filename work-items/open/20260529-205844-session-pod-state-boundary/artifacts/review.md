# Review: session-pod-state-boundary

Verdict: blocking

## Conceptual summary

The crate split is mostly in the intended shape: `pod-store` now owns the Pod metadata types, validation, trait, and filesystem-backed store; `session-store` no longer exports Pod metadata; normal construction uses separate session and Pod-state roots; TUI discovery goes through `PodMetadataStore`; and the old `{sessions_root}/pods` layout is not used as an authority.

The late scope-authority change is only partially satisfied. `pod.scope` session-log authority appears removed, and restore derives initial deny rules from `pod-store` `spawned_children`, but the restore reconciliation path has a runtime-state hole for missing children. That violates the ticket's requirement to reclaim delegated scope for missing/stopped/unreachable children before resumed model use.

## Blocking issues

1. **Restore reconciliation leaves stale runtime deny rules when the missing child has no live lock allocation.**

   - `crates/pod/src/pod.rs:4597-4616` derives restored parent scope by adding deny rules for every outstanding `metadata.spawned_children` write delegation.
   - `crates/pod/src/pod.rs:3960-3967` installs the restored top-level Pod into the runtime registry with those deny rules.
   - `crates/pod/src/spawn/registry.rs:131-159` then detects unreachable children and calls `pod_registry::reclaim_delegated_scope(...)` via `reclaim_record(...)`.
   - But `crates/pod-registry/src/mutate.rs:202-215` removes the parent's `scope_deny` entries only when the child allocation currently exists (`if child_exists { ... remove parent deny ... }`). If the child is missing from the live lock registry—which is exactly one of the required restore-reconciliation cases—the restored parent allocation keeps the delegated write deny in `pods.json` even though `pod-store` has moved the child to `reclaimed_children` and the in-memory `SharedScope` has removed the deny.

   This makes runtime lock state diverge from `pod-store`/in-memory scope after restore. Future spawn/delegation checks that consult the lock allocation will still see the reclaimed write scope as denied. The existing test `load_from_pod_state_reclaims_pruned_child_scope_and_records_history` only covers the case where a stale child allocation exists, so it misses the required "missing child" case. The reclaim operation needs to remove the parent's matching delegated deny independently of whether the child allocation still exists, while still being idempotent.

2. **Restore reconciliation is not structurally part of `Pod::restore_*`; it depends on `PodController::spawn`.**

   `Pod::restore_from_manifest` / `Pod::restore_from_pod_metadata` construct a restored Pod with scope derived from outstanding `pod-store` delegations, but the pruning/reclaim and notification are performed later in `PodController::spawn` (`crates/pod/src/controller.rs:160-174`). Normal CLI startup goes through the controller, but the public restore constructors can return a Pod that can be used directly without the required reconciliation notification. If direct use is intentionally unsupported, the API should make that boundary explicit; otherwise the reconciliation should be moved into or made mandatory by the restore path so "before any model request observes resumed state" is enforced by construction.

## Non-blocking notes

- `pod-store::PodMetadataStore::update_by_name` is still implemented as a read-modify-write default method (`crates/pod-store/src/lib.rs:133-145`). The call sites no longer manually preserve unrelated fields, and the field-specific methods are test-covered, so this is acceptable for the current filesystem backend. If concurrent metadata writers become possible, this API will need a stronger atomicity story.
- `pod-store` depends on `session-store` for `SessionId`/`SegmentId`. The ticket allowed a narrow dependency for IDs; the crate docs keep the replay boundary clear.
- `crates/pod/src/main.rs:230-235` derives the Pod-state root from `paths::data_dir()` rather than from custom `--store` when data dir is available. That keeps the new top-level layout, but users of a custom session store do not have a matching custom Pod store flag yet.

## Validation run

Read/inspected:

- Ticket: `work-items/open/20260529-205844-session-pod-state-boundary/item.md`
- Implementation diff/stat for commits `2117381..e10b4ad`
- Key files: `crates/pod-store/src/lib.rs`, `crates/session-store/src/lib.rs`, `crates/session-store/src/fs_store.rs`, `crates/pod/src/pod.rs`, `crates/pod/src/controller.rs`, `crates/pod/src/spawn/registry.rs`, `crates/pod-registry/src/mutate.rs`, `crates/tui/src/pod_list.rs`, `crates/pod/src/main.rs`

Commands run from `/home/hare/Projects/insomnia/.worktree/session-pod-state-boundary`:

```text
cargo test -p pod-store
cargo test -p pod --test pod_comm_tools_test load_from_pod_state_reclaims_pruned_child_scope_and_records_history
cargo test -p session-store
cargo test -p tui pod_list
git diff --check HEAD~2..HEAD
./tickets.sh doctor
```

All commands passed. Full command output was saved by the shell tool at `/run/user/1000/insomnia/review-session-pod-state-boundary/bash-output/bash-uDx0E4.log`.
