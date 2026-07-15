Ticket configuration の Workspace settings 統合を実装・レビュー・merge・検証した。

実装内容:
- durable Ticket policy authority を `.yoi/workspace.toml [ticket]` に移動。
- tracked `.yoi/ticket.config.toml` を削除。
- Workspace `[ticket]` がない場合のみ legacy `.yoi/ticket.config.toml` を narrow read-only migration fallback として読むようにした。
- Workspace `[ticket]` が存在する場合は legacy config を無視し、二重 active authority を避ける precedence にした。
- default behavior は維持し、設定なしでは `provider = builtin:yoi_local`, `root = .yoi/tickets` を使う。
- Ticket backend provider/root、record language、orchestration defaults、fixed role launch config を Workspace settings authority に移した。
- Direct Ticket CLI / Objective config consumers / Worker Ticket feature setup / workspace-server Ticket backend endpoint / project record reader / Panel role launch availability / role launch tests を新 authority 解決へ更新。
- Panel orchestration overlay も orchestration worktree の `TicketConfig::load_workspace(worktree_root)` を使い、configured backend root と record language を尊重するよう修正。
- docs で `.yoi/ticket.config.toml` obsolete と新 `[ticket]` workspace settings shape を記録。

Review:
- 初回 review は Panel orchestration overlay が `.yoi/tickets` を hard-code している blocker で `request_changes`。
- `40f21145 tui: use configured ticket root for overlay` で overlay read path を configured Ticket root に移行し、non-default root regression test を追加。
- focused re-review は `approve`。

Merge / validation:
- Merge commit: `45783b74 merge: workspace ticket settings`。
- Final validation passed:
  - `cargo fmt --check`
  - `git diff --check`
  - `cargo test -p ticket`
  - `cargo test -p yoi --tests`
  - `cargo test -p client --lib --tests`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p tui --lib --tests`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/workspace-ticket-settings-final-validation-1784137566.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。