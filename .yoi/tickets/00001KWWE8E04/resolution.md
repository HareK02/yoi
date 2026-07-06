完了。

実装内容:
- Workspace Browser の canonical UI routes を `/w/<workspace-id>/...` に移行した。
- Browser-facing Workspace API に `/api/w/<workspace-id>/...` scoped variants を追加した。
- Backend current immutable `workspace_id` と path `workspace-id` を照合し、mismatch は typed sanitized `workspace_id_mismatch` diagnostic / 404 として fail closed するようにした。
- Frontend の workspace-scoped data calls は `/api/w/<workspace-id>/...` helper 経由に変更した。
- Sidebar / settings / repository / objectives / worker console links は workspace id を含む scoped links を使うようにした。
- Runtime `/v1/...` API surface は変更しなかった。
- Unscoped browser routes は `/api/workspace` bootstrap から scoped `/w/<workspace-id>/...` へ redirect する thin shim とし、通常の unscoped data route / duplicated co-canonical page surface を廃止した。
- Backend SPA fallback でも unscoped Browser routes を scoped route に redirect するようにした。
- Focused backend/web tests を追加/更新した。

主な commit / merge:
- implementation: `f6ad9cfc feat: scope workspace routes by id`
- review fix: `71340b89 fix: redirect unscoped workspace routes`
- merge into orchestration: `13e756e7 merge: workspace id route scoping`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `cargo test -p yoi-workspace-server`: pass（65 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `yoi ticket doctor`: ok

未実行:
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。