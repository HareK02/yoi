Ticket `00001KVDH2E06` is complete.

Completed implementation:
- Normal `yoi panel` Pod rows and Pod action targets are filtered to the current runtime workspace using persisted Pod metadata / runtime workspace ownership.
- Added optional persisted `workspace_root` to `PodMetadata` and ensured normal metadata write-through persists it.
- Added `PodMetadataStore::set_active_with_workspace_root(...)` while keeping compatibility `set_active(...)` preserving existing workspace ownership.
- Workspace-external Pods are excluded from normal Panel rows/action targets.
- Unknown/corrupt/live-only metadata fail closed and are not treated as current workspace Pods.
- Current-workspace role-like Pods remain visible when persisted runtime workspace matches, independent of cwd/worktree differences.

Reviewed / merged:
- Initial implementation: `3b634d66` (`tui: filter panel pods by workspace`)
- Persistence-boundary fix: `160c96ad` (`pod: persist metadata workspace root`)
- First review requested changes for missing persistence of `workspace_root` through the normal store writer path.
- Re-review approved with no remaining blockers.
- Orchestrator merge commit: `0ef36b4e` (`merge: panel workspace pod filter`)

Validation in Orchestrator worktree:
- `cargo fmt --check` — passed
- `cargo check -p tui -p pod -p pod-store` — passed
- `cargo test -p pod metadata_writer_persists_workspace_root_through_store_update -- --nocapture` — passed
- `cargo test -p pod pod_metadata -- --nocapture` — passed
- `cargo test -p pod-store` — passed; 6 passed, 0 failed
- `cargo test -p tui workspace_panel -- --nocapture` — passed; 23 passed, 0 failed
- `cargo test -p tui pod_list -- --nocapture` — passed; 19 passed, 0 failed
- `git diff --check` — passed

Known validation debt:
- Full `cargo test -p tui` still has three pre-existing/unrelated failures reproduced on the Orchestrator branch before this implementation was merged:
  - `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`
  - `spawn::tests::profile_choices_use_project_registry_default`
  - `spawn::tests::profile_choices_include_builtin_and_project_default_marker`

Cleanup:
- Stopped Coder Pod `yoi-coder-00001KVDH2E06`.
- Stopped Reviewer Pod `yoi-reviewer-00001KVDH2E06-r2`.
- Removed child worktree `/home/hare/Projects/yoi/.worktree/00001KVDH2E06-panel-current-workspace-pods`.
- Deleted merged branch `impl/00001KVDH2E06-panel-current-workspace-pods`.

Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction. The completed work is integrated on the Orchestrator branch.