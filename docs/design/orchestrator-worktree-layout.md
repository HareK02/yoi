# Orchestrator worktree and implementation worktree layout

Yoi can run an Orchestrator from a dedicated orchestration worktree while implementation worktrees still belong under the original workspace root. The runtime must therefore distinguish role execution cwd/workspace from the target repository root used for implementation branches and merge-completion.

This decision records the layout policy for Ticket `00001KTTB479X`.

## Roots

- `role_workspace_root`: the launched role runtime workspace root and Ticket backend context.
- `original_workspace_root`: the repository root from which implementation worktrees should be created.
- `implementation_worktree_root`: `<original_workspace_root>/.worktree`.
- `merge_target_workspace_root`: the workspace/branch where merge-completion validates and merges.

When these roots differ, implementation worktrees must be created under `implementation_worktree_root`, not under the Orchestrator process cwd. Merge-completion must run against `merge_target_workspace_root`, not whichever worktree happens to host the Orchestrator.

## Policy

- The filesystem Ticket backend remains authoritative for project records; this design does not introduce a Git-external Ticket store.
- Main-workspace draft Tickets are not an implicit Orchestrator queue. Queue acceptance still requires explicit Ticket state and routing checks.
- Implementation branches should not include unrelated Orchestrator Ticket churn. Orchestration progress stays in the Orchestrator workspace/Ticket backend; child implementation worktrees contain implementation diffs and branch-local artifacts only when needed.
- Prompt and workflow guidance must treat merge-ready dossier as a checkpoint. If reviewer approval, clean/safe target workspace, and standing/user merge authority are present, Orchestrator should continue through merge, validation, Ticket close, and cleanup instead of stopping solely because a dossier exists.

## Implemented in this branch

- Ticket role launch prompts now emit a `Workspace routing context` section for Orchestrator launches, and for any launch that explicitly provides original/target roots.
- `TicketRoleLaunchPlan` carries `original_workspace_root`, `implementation_worktree_root`, and `target_workspace_root` as explicit plan fields for callers/tests.
- Orchestrator role prompt resources now say to create implementation worktrees under the original workspace root and to run merge-completion against the recorded target workspace.
- `worktree-workflow` now describes `<original-workspace-root>/.worktree/<task-name>` and `git -C <original-workspace-root> ...` instead of assuming the Orchestrator cwd is the repository root.

## Follow-up boundaries

- Panel/workspace orchestration can later populate `original_workspace_root` and `target_workspace_root` when launching Orchestrator from a dedicated worktree.
- A future implementation can add a first-class orchestration-worktree launcher and cleanup policy, but it must keep the four-root distinction above visible in launch diagnostics and tests.
