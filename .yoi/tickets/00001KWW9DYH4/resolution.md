完了。

実装内容:
- Runtime-side working directory materialization boundary を追加した。
- v0 local Git materializer として、configured repository から resolved commit / tree evidence を取り、Runtime root 配下 `working-directories/<allocation-id>/root/<repository-id>` に detached Git worktree を作成するようにした。
- Worker spawn 時の workspace root / cwd / scope は source repository root ではなく materialized worktree path を使うようにした。
- 同一 source repository に対する複数 Worker が distinct materialized paths を使えるようにした。
- dirty source state は `clean_point_only` policy で拒否し、remote URI / non-Git provider / unsupported policy は typed diagnostics で fail closed するようにした。
- allocation evidence / cleanup target / cleanup policy / status を記録し、Worker stop cleanup hook で worktree cleanup と record update を行うようにした。
- Browser/API-facing spawn boundary は raw `WorkingDirectoryRequest` / `local_path` を受け取らないようにし、safe `repository_id` / optional `selector` から host/server 内部で materialization request を構築するようにした。
- Focused worker-runtime / workspace-server tests を追加した。

主な commit / merge:
- implementation: `8b7a5da0 feat: materialize working directories`
- review fix: `c90ebf24 fix: keep working directory paths internal`
- merge into orchestration: `c4cdf1c3 merge: working directory materializer`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（34 lib tests + 5 main tests + doc tests）
- `cargo test -p yoi-workspace-server`: pass（65 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（15 tests）
- `yoi ticket doctor`: ok

未実行:
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。