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
