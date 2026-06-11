完了しました。

実施内容:
- Panel Orchestrator spawn path で、original workspace root 配下に専用 orchestration worktree を自動作成・再利用するようにしました。
  - path: `<original-workspace-root>/.worktree/orchestration/<workspace-orchestrator-pod-name>`
  - branch: `orchestration/<workspace-orchestrator-pod-name>`
- clean な既存 orchestration Git worktree は reuse します。
- unsafe state は削除せず diagnostic で拒否します。
  - path が directory でない
  - Git worktree でない
  - expected branch でない
  - original workspace と Git common dir が異なる
  - inherited parent worktree で expected path 自体が worktree top-level でない
  - dirty orchestration worktree
- Orchestrator の `SpawnConfig.workspace_root` は orchestration worktree root になります。
- `TicketRoleLaunchContext` には original workspace root / target workspace root を渡し、prompt 上でも original / implementation / target roots が見える形を維持します。
- Panel diagnostic に created/reused worktree path/branch または failure reason を出すようにしました。
- tests を追加・更新しました。
  - stable layout naming
  - root population
  - invalid existing path no-cleanup
  - create/reuse
  - dirty reuse refusal
  - wrong branch refusal
  - unrelated repo refusal
  - inherited parent worktree refusal
  - existing client prompt/spawn root behavior

Merge:
- Branch: `panel-orchestration-worktree`
- Implementation commit: `00e11b3d feat: launch orchestrator from worktree`
- Merge commit: `735b0c04 merge: panel orchestration worktree`

確認:
- Branch-local reviewer `reviewer-panel-orchestration-worktree` が2回 request_changes 後、修正済み branch を approve。
- `cargo fmt --check` passed。
- `cargo test -p tui orchestration --lib` passed。
- `cargo test -p tui existing_ --lib` passed。
- `cargo test -p tui inherited_parent_worktree_directory_is_rejected_without_cleanup --lib` passed。
- `cargo test -p client ticket_role --lib` passed。
- `git diff --check` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。今後の改善として、Panel 表示上の orchestration worktree status をより詳細に常時表示することは follow-up として扱えます。