完了しました。

実施内容:
- `yoi setup-model` を top-level command として追加しました。
- setup path は通常の Pod 起動/attach/session 復元とは分離され、選択した catalog-backed model を user config 配下の Profile 設定として保存します。
- `profiles.toml` の default selector と `[profile.default]`、および generated `profiles/default.lua` を deterministic に書きます。
- setup 実行中に workspace `.yoi`、Ticket、session、runtime/local/secret-like files は書きません。
- `yoi --help` に `yoi setup-model` を表示します。
- `package.nix` cargoHash も更新しました。

Merge:
- Branch: `tui-model-setup-wizard`
- Merge commit: `021661b5 merge: setup model wizard`

確認:
- Branch-local reviewer `reviewer-tui-model-setup-wizard` が approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p tui setup_model --lib` passed。
- `cargo test -p yoi setup_model --bin yoi` passed。
- `cargo check -p yoi` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。将来的に richer alt-screen setup UI に発展させる余地はありますが、本 Ticket の one-shot setup command / Profile persistence 要件は満たしています。