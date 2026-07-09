完了。

実装内容:
- Backend-owned `ProfileSourceTree` virtual filesystem model を追加した。
- virtual relative paths / safe source tree ids / file ids / revision / content digest / content type / typed provenance / diagnostics を扱う safe summaries を追加した。
- Profile source tree list/read/write/create/delete/validate scoped APIs を追加した。
- virtual import resolver を追加し、relative imports / scoped namespace imports を扱い、absolute path、URL/unsupported scheme、root escape、unsupported namespace、limit excess を fail closed するようにした。
- `ProfileSourceArchive` construction を selected root/selector + source tree revision + import closure snapshot に変更し、unrelated sources を archive に含めないようにした。
- Runtime `ArchiveSourceLoader` / archive verification を virtual path / import-map only に強化し、Workspace filesystem/backend fallback を導入しないまま維持した。
- Settings UI に Profile source tree list と Decodal source editor route を追加した。
- `decodal-codemirror@0.1.2` と CodeMirror 6 の `decodal()` extension を editor に接続した。
- UI create/save/delete operations を Backend virtual-path APIs に接続した。
- `.yoi/profiles` source root symlink と symlink/path escape を list/read/write/delete/archive construction paths で reject し、outside filesystem mutation 前に fail closed するようにした。
- Focused backend/runtime/web tests を追加した。

主な commit / merge:
- implementation: `81abfa63 feat: add profile source tree editor`
- review fix: `941b9126 fix: complete profile source tree review fixes`
- symlink-root fix: `13c6548b fix: reject profile source root symlinks`
- merge into orchestration: `9a2e57fb merge: profile source tree editor`

Review:
- 初回 external review は request_changes。
- first follow-up review も request_changes（source-root symlink blocker）。
- second follow-up review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p yoi-workspace-server`: pass（94 lib tests + 2 main tests）
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（47 lib tests + 5 main tests + doc tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（19 tests）
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass

補足:
- implementation branch merge 時に Ticket record conflict が発生したため、Orchestrator 側の Ticket item/thread を保持して解決した。product/code changes は merge 済み。
- Reviewer の non-blocking follow-up: `read_profile_tree_file` に対する root-symlink direct assertion を追加すると regression matrix がさらに明示的になるが、同じ guarded `checked_source_path` を通るため merge blocker ではない。