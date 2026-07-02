完了。

実装内容:
- Workspace Settings に Runtime Connections v0 を追加し、embedded Runtime を built-in / delete不可として表示するようにした。
- remote Runtime connection の list/add/delete/test を Browser-facing API と Settings UI から扱えるようにした。
- remote runtime config は `.yoi/workspace-backend.local.toml` の既存 `[[runtimes.remote]]` schema を typed read-modify-write で更新し、変更後は `restart_required = true` を返すようにした。
- Runtime connection test は `/v1/runtime` の lightweight negotiation と Browser-relevant operation の compatibility/capability projection を行い、unsupported/unknown を typed sanitized diagnostics として返すようにした。
- Browser-facing response から raw token/secret、Runtime endpoint、config path、workspace/internal paths、socket/session/store path、live handle 等が漏れないようにした。
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