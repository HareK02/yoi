Ticket role launch の first-run user message を短縮し、role behavior / procedural guidance / control-plane metadata の所管を分離した。

実装概要:
- `crates/client/src/ticket_role.rs` の launch Submit text を短縮し、対象 Ticket / action instruction / per-launch context に限定した。
- `Profile selector`、`Workflow`、`Configured launch_prompt ref`、Ticket record language guidance、runtime workspace/cwd などの control-plane / environment 情報を first-run user text から外した。
- builtin role Profiles に `worker.instruction = "$yoi/role/<role>"` を設定し、role behavior を `resources/prompts/role/{intake,orchestrator,coder,reviewer}.md` へ移した。
- procedural guidance は Workflow resources 側に保持・移動した。
- obsolete な `resources/prompts/ticket_role/*.md` fragments を削除した。
- boundary を検証する focused unit tests を追加・更新した。

Review / integration:
- Implementation commit: `949531a0 client: shorten ticket role launch input`
- Reviewer: `yoi-reviewer-role-launch-input` が approve。
- Orchestrator merge commit: `bdbd955b merge: ticket role launch input split`
- Ticket completion commit: `9ad5ed6d ticket: mark role launch done`

Validation:
- `cargo test -p client ticket_role --lib`: pass
- `cargo test -p manifest builtin_role_profiles_are_registered_and_resolve --lib`: pass
- `cargo test -p pod builtin_ticket_role_instructions_resolve --lib`: pass
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `nix build .#yoi`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input` removed。
- branch `ticket/shorten-ticket-role-launch-input` deleted。

Non-blocking follow-up:
- 旧 `.yoi/prompts/ticket_role/*` override は launch Submit text には効かなくなるため、外部ユーザー向けには role behavior は Profile instruction、手順は Workflow override へ移す旨を必要に応じて案内すると安全。