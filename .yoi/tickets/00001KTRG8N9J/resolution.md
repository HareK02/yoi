完了しました。

実施内容:
- `builtin:companion` / `builtin:intake` / `builtin:orchestrator` / `builtin:coder` / `builtin:reviewer` を builtin Profile として追加しました。
- role Profile は `resources/profiles/*.lua` に移動し、global `yoi` style の standalone `yoi.profile { ... }` artifact として登録しました。
- `builtin:default` 由来の model ref / web secret / workspace write scope が role contract に混入しないよう、role Profiles は `builtin:default` を extend しない形にしました。
- `companion` / `intake` / `orchestrator` / `reviewer` は read scope、`coder` のみ write scope、`orchestrator` は reusable delegation intent として `delegation_scope = workspace_write()` を持ちます。
- `.yoi/ticket.config.toml` の role selectors を `project:*` から `builtin:*` に移行しました。
- project-local role Profile files を削除し、`.yoi/profiles.toml` は workspace default を `builtin:companion` に向けるだけに整理しました。
- Ticket config scaffold と client role launch tests を role-specific builtin defaults に合わせて更新しました。
- manifest tests に builtin role registry/resolution と role policy boundary の検証を追加しました。

Merge:
- Branch: `builtin-role-profiles`
- Implementation commit: `85c06dc6 feat: add builtin role profiles`
- Merge commit: `7daecca8 merge: builtin role profiles`

確認:
- Branch-local reviewer `reviewer-builtin-role-profiles` が初回 request_changes 後、修正済み branch を approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p manifest profile --lib` passed。
- `cargo test -p ticket config --lib` passed。
- `cargo test -p client ticket_role --lib` passed。
- `cargo check -p manifest -p ticket` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。将来、role builtin Profile とは別に broad user/project profile import selector が必要になった場合は follow-up Ticket として扱えます。