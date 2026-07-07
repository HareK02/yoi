完了。

実装内容:
- Browser-managed Execution Workspace API を scoped Workspace API に追加した。
  - `POST /api/w/<workspace-id>/execution-workspaces`
  - `GET /api/w/<workspace-id>/execution-workspaces`
  - detail / cleanup endpoint
- Browser create payload は configured `repository_id` / selector / policy を使い、raw host path / raw internal `ExecutionWorkspaceRequest` / internal runtime path を受け取らないようにした。
- Browser response は allocation id、repository id、selector/commit evidence、status、cleanup policy など safe summary のみを返すようにした。
- Worker launch UI で Execution Workspace を create/select できるようにした。
- Worker spawn は selected allocation id + optional `relative_cwd` を使い、Runtime materialized workspace に接続するようにした。
- `relative_cwd` は materialized root 相対として検証し、absolute path、`..` escape、symlink escape、nonexistent / non-directory を拒否するようにした。
- invalid `relative_cwd` は Browser-facing API で typed `execution_workspace_relative_cwd_*` diagnostic として返るようにした。
- Worker list/detail に execution workspace safe summary を含めた。
- cleanup は allocation root に bounded され、typed/sanitized diagnostic を返すようにした。
- Focused backend/runtime/web tests を追加した。

主な commit / merge:
- implementation: `684b19e8 feat: add browser execution workspaces`
- review fix: `9adb0fae fix: preserve execution workspace cwd diagnostics`
- merge into orchestration: `b52986e5 merge: browser execution workspaces`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `cargo test -p yoi-workspace-server`: pass（67 lib tests + 2 main tests）
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（36 lib tests + 5 main tests + doc tests）
- `cargo check -p yoi`: pass
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass