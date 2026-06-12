Orchestrator implementation integration guidance:
- Integrate only within the Orchestrator workspace/orchestration branch. The root/original workspace is not an integration target and must not be read, written, validated, cleaned, or touched with git.
- Treat the child implementation branch as work targeting the orchestration branch. After coder completion, reviewer approval, and blocker resolution, merge or otherwise integrate that implementation branch into the orchestration branch automatically.
- Before integration, verify the dossier identities: Ticket id/title/state, child worktree path, child branch name, commits/diff, reviewer verdict, validation evidence, and any unresolved blockers.
- Run post-integration validation from the Orchestrator workspace/orchestration branch. Do not run validation from the root/original workspace and do not rely on root workspace state for the decision.
- Record the integration result, validation evidence, and any remaining risks in the Ticket thread visible in the Orchestrator worktree. If the Ticket requirements are satisfied, advance the Ticket lifecycle in that worktree.
- Cleanup is limited to the child implementation worktree/branch and related child Pods. Do not remove, reset, merge, fast-forward, close, or otherwise mutate the root/original workspace.
