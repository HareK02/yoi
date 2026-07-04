完了。

実装内容:
- Workspace Backend に explicit `[[repositories]]` registry を追加した。
- `.yoi/workspace-backend.local.toml` / `resources/workspace-backend.default.toml` に Repository config schema を導入し、dogfood workspace config に explicit `main` Git repository entry (`uri = "."`) を追加した。
- `uri = "."` は workspace config root 相対として解釈し、process cwd / workspace root を暗黙 Repository として扱う fallback を廃止した。
- `/api/repositories` は configured repositories のみを返し、未設定時は empty list + `repository_config_empty` warning diagnostic を返す。
- `/api/repositories/{id}/log` は configured Git repository id のみを受け付け、未知 id / unsupported provider は typed sanitized diagnostics で fail closed するようにした。
- Browser-facing repository summary は configured `uri` / resolved local path / secret / internal path を露出せず、Git remote も credentials / local absolute path / `file://...` を redaction するようにした。
- Web UI は configured repositories から navigation を導出し、empty registry で hardcoded `main` repository を invent しないようにした。
- Focused backend/web tests を追加した。

主な commit / merge:
- implementation: `2f0d1cee feat: add workspace repository registry`
- review fix: `14e63ca5 fix: derive repository UI from registry`
- merge into orchestration: `a786fd85 merge: repository registry implementation`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p yoi-workspace-server`: pass（63 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（15 tests）
- `yoi ticket doctor`: ok

未実行:
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。