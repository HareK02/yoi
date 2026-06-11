完了しました。

実施内容:
- `docs/design/orchestrator-worktree-layout.md` を追加し、`role_workspace_root` / `original_workspace_root` / `implementation_worktree_root` / `merge_target_workspace_root` の4-root model を記録しました。
- `TicketRoleLaunchContext` / `TicketRoleLaunchPlan` に original/target/worktree root を追加しました。
- Orchestrator launch prompt に `Workspace routing context` を出し、role runtime workspace / Ticket backend root と original implementation root / merge target root を区別できるようにしました。
- Orchestrator routing / merge-completion prompt resources を更新し、implementation worktree は recorded original workspace root の `.worktree` 配下、merge/cleanup は recorded merge target workspace で行うよう明示しました。
- `.yoi/workflow/worktree-workflow.md` と `.yoi/workflow/multi-agent-workflow.md` を更新し、Orchestrator cwd を original repo root / merge target とみなさない方針に揃えました。
- standing merge authority があり、reviewer approval / safe target workspace / no blockers の条件が揃う場合は merge-ready dossier で止まらず merge/validation/close/cleanup まで進む guidance にしました。
- client Ticket-role tests を更新し、root fields と `SpawnConfig.workspace_root` が role runtime workspace root のまま保たれることを検証しました。

Merge:
- Branch: `orchestrator-worktree-layout`
- Implementation commit: `e7c78f96 feat: track orchestration workspace roots`
- Merge commit: `5f7b3015 merge: orchestrator worktree layout`

確認:
- Branch-local reviewer `reviewer-orchestrator-worktree-layout` が初回 request_changes 後、修正済み branch を approve。
- `cargo fmt --check` passed。
- `cargo test -p client ticket_role --lib` passed（18 passed）。
- `git diff --check` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- Panel/workspace orchestration が dedicated Orchestrator worktree から起動する際に `original_workspace_root` / `target_workspace_root` を実際に populate する経路は follow-up 境界です。