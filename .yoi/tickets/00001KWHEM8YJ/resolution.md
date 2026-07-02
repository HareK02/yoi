完了。

実装内容:
- Workspace Sidebar の WORKER heading に `New` button と Worker 作成 form を追加した。
- form は display name / runtime / profile / initial text の product-level fields のみを扱う。
- Browser-facing `/api/workers` POST を追加/利用し、内部 Runtime create payload や raw authority-bearing fields を UI/API contract に露出しないようにした。
- profile/runtime は Backend-published candidates から選択し、自由入力にしないようにした。
- 作成成功時は Worker list を refresh し、`/runtimes/{runtime_id}/workers/{worker_id}/console` に遷移するようにした。
- unsupported runtime / not accepted cases は Runtime diagnostics を sanitized typed diagnostics として保持するようにした。
- Focused backend/web tests を追加した。

主な commit / merge:
- implementation: `f2fead7e feat: add workspace runtime and worker controls`
- review fix: `47ed0ff8 fix: harden runtime and worker launch controls`
- merge to develop: `4edaa73d merge: runtime worker controls`
- ticket-record merge before closure: `540e55d4 merge: runtime worker ticket records`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation on `develop`:
- `git diff --check`: pass
- `cargo test -p yoi-workspace-server`: pass（55 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task test`: pass（13 tests）
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `yoi ticket doctor`: ok

未実行:
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。