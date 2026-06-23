Local Workspace identity を tracked `.yoi/workspace.toml` に永続化する実装を完了した。

完了内容:
- `.yoi/workspace.toml` を local Workspace identity の authority として追加。
- v0 schema は `workspace_id`, `created_at`, `display_name` のみ。
- `workspace_id` は UUIDv7 canonical string として検証し、path / display basename から導出しない。
- `created_at` は UTC RFC3339 timestamp として検証。
- `display_name` は空文字列を拒否。
- unknown fields は v0 では明示的に deny。
- invalid existing file は fail closed し、自動上書きしない。
- missing file 初期化では `OpenOptions::create_new(true)` を使い、既存 file が race で作られた場合は persisted identity を read/parse して返す。
- fixed temp file / `path.exists()` + `rename` finalization は使わない。
- `ServerConfig::local_dev` は `local:{display}` を生成せず、persisted `WorkspaceIdentity` を受け取る。
- SQLite `workspaces` upsert、Workspace API、repository ids、host ids は persisted Workspace id を使う。
- legacy `/api/repositories/local` alias は維持。
- 既存 `local:*` DB rows の destructive migration はしない。
- `.yoi/workspace.toml` は絶対 path / secret / socket / runtime data を含まない安全な project identity record として維持。

統合:
- Implementation: `31565c9b feat: persist workspace identity`
- Fix: `49c9e190 fix: return persisted workspace identity`
- Merge: `2745f3d5 merge: workspace identity persistence`

検証:
- Reviewer approval: `yoi-reviewer-00001KVSKGDYS-r1`
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && deno task check && deno task build`: passed
- `cargo run -q -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

生成物 cleanup:
- orchestration worktree の `target/`, `web/workspace/node_modules/`, `web/workspace/.svelte-kit/`, `web/workspace/build/` を削除済み。

残作業:
- なし。将来必要なら legacy `local:*` row cleanup / migration policy は別 Ticket で扱う。