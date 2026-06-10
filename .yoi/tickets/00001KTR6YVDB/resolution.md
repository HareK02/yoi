完了しました。

実施内容:
- Branch `prompt-resource-centralization` を `develop` に `--no-ff` merge しました。
- Merge commit: `9aaa3232 merge: prompt resource centralization`
- LLM 向け Ticket role launch prompt prose は `crates/client/src/ticket_role.rs` の production hardcoding から `resources/prompts/ticket_role/*.md` へ移動されました。
- Rust 側は runtime dynamic fields の sectioned/bounded composition と workspace/user/builtin prompt resource lookup を担当します。
- Workspace prompt override の regression coverage が追加されています。

確認:
- Branch-local reviewer `reviewer-prompt-resource-centralization` が approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p client ticket_role --lib` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。user-level prompt override 専用 test は将来の追加余地として non-blocking note に留めました。