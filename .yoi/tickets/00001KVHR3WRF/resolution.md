## Resolution

`00001KVHR3WRF` を完了しました。

実装内容:
- Typed MCP config schema を `crates/manifest` に追加しました。
- Profile/config で named local stdio MCP server を宣言できるようにしました。
- Config fields は `name`, `command`, `args`, `cwd`, `env.inherit`, `env.set` を含みます。
- Env value は `literal`, `secret_ref`, `env_ref` をサポートします。
- Validation は duplicate names、empty/NUL command/args、cwd policy/path、env var name、secret ref、NUL literal env values などを fail-closed で検査します。
- Diagnostics / `Debug` は secret/env/literal values を plaintext で出さないよう redaction します。
- Profile resolution / child manifest inheritance に MCP config を通しましたが、subprocess spawning / initialize / JSON-RPC lifecycle / tool/resource/prompt registration は実装していません。
- Docs に local stdio MCP server の trust boundary を記録しました。Configured stdio server は user OS permissions で動く local executable であり、Yoi feature authority / Plugin permissions / MCP config validation は OS sandbox ではありません。

主な commit:
- `e0680cce mcp: add stdio server config`
- `9b7c4e27 merge: mcp stdio config trust`

Review:
- r1 は `approve`。
- Reviewer は config-only boundary、no process spawning/no auto-start、secret redaction、Profile/config integration、docs trust boundary を確認しました。

最終 validation:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p manifest mcp --lib`
- `cargo check`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `112615056`

Known unrelated note:
- Full `cargo test -p manifest --lib` は、branch 外の既存 Plugin template-shape mismatch で失敗するため最終 gate にしませんでした。Reviewer はこの failure が `b0225e48..HEAD` の diff に起因しないことを確認済みです。

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-uxMpR3.log`