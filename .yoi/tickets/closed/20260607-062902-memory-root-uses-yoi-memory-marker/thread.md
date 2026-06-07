<!-- event: create author: LocalTicketBackend at: 2026-06-07T06:29:02Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T07:44:53Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready after `narrow-yoi-worktree-sparse-exclusions` landed.

Intent:
- Keep `.yoi` as the Yoi project records/workspace marker.
- Stop using `.yoi` alone as the memory root marker.
- Resolve repo-local memory through an explicit `memory.workspace_root` or an ancestor `.yoi/memory` marker, so child worktrees can contain `.yoi/tickets`/`.yoi/workflow` without becoming independent memory roots.

Current code map:
- `crates/memory/src/workspace.rs`
  - `WorkspaceLayout::resolve(cfg, default_root)` currently uses `cfg.workspace_root.unwrap_or(default_root)`.
  - `memory_dir()` is `<root>/.yoi/memory`; `knowledge_dir()` and `workflow_dir()` are also rooted under `<root>/.yoi`.
  - Tests currently assert fallback to default root.
- `crates/pod/src/pod.rs`
  - Memory extract/consolidation/tools/resident setup call `WorkspaceLayout::resolve(&memory_cfg, &self.pwd)`.
- `crates/yoi/src/memory_lint.rs`
  - CLI lint works with explicit workspace path and may need to keep explicit workspace behavior.
- `manifest::MemoryConfig` currently uses `Option<MemoryConfig>` presence as enablement and `workspace_root: Option<PathBuf>` as the explicit root. There is not yet a separate `enabled = false` field.

Implementation direction:
- Add a small resolver in `memory::WorkspaceLayout` or nearby that:
  - honors explicit `memory.workspace_root` exactly as today;
  - when no explicit root exists, walks ancestors from the default root and selects the nearest ancestor that contains `.yoi/memory`;
  - does not select a directory merely because it contains `.yoi`;
  - if no ancestor `.yoi/memory` exists, uses an explicit, documented fallback that does not silently interpret `.yoi` alone as the memory root. Prefer fail/no-marker behavior for automatic memory operations over creating child-worktree memory; if a lazy-create path is needed, keep it tied to explicit configuration rather than implicit `.yoi` presence.
- Preserve `.yoi/knowledge`/`.yoi/workflow` project-record lookup semantics as much as possible; be explicit if moving memory root also changes knowledge/workflow layout root in a way that needs follow-up.
- Do not implement user-data overlay fallback unless it is already supported and clearly smaller.
- If adding a manifest `enabled` field would be broad, do not do it in this slice; document that current explicit disable is represented by absence of `[memory]` and keep the ticket focused on `.yoi/memory` marker resolution.

Tests to add/update:
- Explicit `workspace_root` still wins.
- No explicit root + ancestor `.yoi/memory` selects that ancestor.
- Child worktree containing `.yoi/tickets`/`.yoi/workflow` but no `.yoi/memory` does not select the child as memory root.
- `.yoi` alone does not select a memory root.
- Existing explicit workspace/lint behavior remains sensible.

Validation:
- Run `cargo test -p memory workspace --lib` or equivalent focused memory tests.
- Run relevant `cargo test -p pod memory --lib` if Pod memory setup tests are affected.
- Run `cargo test -p yoi memory --lib` if memory lint behavior changes.
- Run `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor`.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T07:45:01Z -->

## Intake summary

Implementation-ready after narrowed worktree sparse exclusions: keep `.yoi` as project records marker, resolve repo-local memory from explicit `memory.workspace_root` or nearest ancestor `.yoi/memory`, and ensure `.yoi` alone does not make child worktrees independent memory roots.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T07:45:01Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T07:45:55Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `memory-root-marker-coder-20260607` in worktree `.worktree/memory-root-uses-yoi-memory-marker` on branch `work/memory-root-uses-yoi-memory-marker`.

The worktree uses the new narrowed sparse rules:
- `.yoi` project records are visible;
- `.yoi/memory` is absent.

Scope:
- Keep `.yoi` as project records marker.
- Resolve memory from explicit `memory.workspace_root` or nearest ancestor `.yoi/memory`.
- Do not treat `.yoi` alone as memory root.
- Do not add broad memory enable/disable redesign.
- Do not change child worktree sparse rules; that has already landed.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T07:53:06Z -->

## Implementation report

## Implementation report

Coder Pod `memory-root-marker-coder-20260607` completed implementation in `.worktree/memory-root-uses-yoi-memory-marker`.

Commit:
- `9ed6613 memory: resolve repo memory by memory marker`

Reported summary:
- Updated `WorkspaceLayout::resolve` to honor explicit `memory.workspace_root` exactly.
- Without explicit root, it searches `default_root` ancestors for the nearest `.yoi/memory` marker.
- `.yoi` alone is never treated as a memory root marker.
- If no marker exists, it falls back to `default_root` as documented no-marker compatibility behavior.
- Added focused tests for explicit root, ancestor marker selection, child worktree project records without memory, and `.yoi`-alone non-marker behavior.
- Updated `MemoryConfig` docs for marker-based implicit resolution.

Reported validation:
- `cargo test -p memory workspace --lib`
- `cargo test -p pod memory --lib`
- `cargo test -p yoi memory` (reported no lib target)
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi`

External review will be delegated before merge.

---

<!-- event: review author: memory-root-marker-reviewer-20260607 at: 2026-06-07T07:58:15Z status: approve -->

## Review: approve

Approved.

Evidence:
- Explicit `memory.workspace_root` behavior is preserved: `WorkspaceLayout::resolve` returns the explicit root before marker search.
- Implicit resolution now searches ancestors for `.yoi/memory` only, using nearest-ancestor order.
- `.yoi` alone is not treated as a memory marker; the resolver never checks `.yoi` by itself, and test coverage verifies project records alone do not define the memory marker root.
- Child worktree behavior is covered: a child `.yoi/tickets` / `.yoi/workflow` without child `.yoi/memory` resolves to the ancestor workspace that has `.yoi/memory`.
- No-marker fallback to `default_root` is acceptable for this slice as documented compatibility behavior and is not based on `.yoi` presence. A stricter no-marker-disabled behavior can be a future enable/disable redesign if desired.
- No broad memory enable/disable redesign or user-data overlay fallback was introduced.

Reviewer validation:
- `cargo test -p memory workspace --lib`
- `cargo test -p pod memory --lib`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi`
- `git merge-tree --write-tree develop HEAD`

Merge readiness: approved; implementation worktree clean and merge-tree against current `develop` succeeded.

---

<!-- event: state_changed author: hare at: 2026-06-07T07:59:04Z from: ready to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T07:59:04Z status: closed -->

## Closed

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

---
