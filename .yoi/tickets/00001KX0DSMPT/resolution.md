完了。

実装内容:
- Workspace/Profile settings scoped API を追加した。
- Workspace metadata read/update と safe identity/source summaries を扱う Settings API を追加した。
- Profile registry/source read/update API を追加し、create/update/delete と revision/etag-style optimistic checks を実装した。
- Backend profile settings model に safe source ids/display paths、Decodal/ProfileSourceArchive validation、selector uniqueness、path/symlink escape protection、source size checks、typed diagnostics を追加した。
- Profile updates 後に Backend profile discovery / validation / launch options を再構築し、Worker launch profile candidates と Settings profile list が同じ discovery result を使うようにした。
- invalid project profile は relevant profile summary に diagnostics を付与し、Worker launch candidates から除外し、candidate validation でも reject するようにした。
- Browser-facing responses/UI から host absolute paths、secret values、Runtime endpoints/tokens、socket/session paths、runtime-local store paths、archive content/digest、raw resource handles、backend-private paths を出さないようにした。
- Settings UI に Workspace overview と Profile list/source editor surface を追加した。
- Focused backend/web tests を追加/強化した。

主な commit / merge:
- implementation: `3eefd333 feat: add workspace profile settings`
- review fix: `0c2ca1ea fix: harden profile settings validation`
- merge into orchestration: `0bf3638c merge: workspace profile settings`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p yoi-workspace-server`: pass（91 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass

補足:
- implementation branch merge 時に Ticket record conflict が発生したため、Orchestrator 側の Ticket item/thread を保持して解決した。product/code changes は merge 済み。