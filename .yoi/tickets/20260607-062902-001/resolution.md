Implemented, reviewed, merged, and validated.

Summary:
- Kept `.yoi` as the Yoi project records marker while stopping `.yoi` alone from acting as the memory root marker.
- `WorkspaceLayout::resolve` still honors explicit `memory.workspace_root` exactly.
- Without explicit root, memory layout resolution now walks ancestors from the default root and selects the nearest ancestor containing `.yoi/memory`.
- Child worktrees containing `.yoi/tickets` / `.yoi/workflow` but no `.yoi/memory` do not become independent memory roots when an ancestor `.yoi/memory` exists.
- `.yoi` project records alone do not define a memory marker root.
- No-marker fallback remains `default_root` as documented compatibility behavior, without relying on `.yoi` presence.
- No broad memory enable/disable redesign or user-data overlay fallback was introduced.
- `MemoryConfig` docs were updated to describe marker-based implicit resolution.

Implementation:
- Child commit: `9ed6613 memory: resolve repo memory by memory marker`
- Merge commit: `merge: memory root marker`

Review:
- External reviewer `memory-root-marker-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p memory workspace --lib`
- `cargo test -p pod memory --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`