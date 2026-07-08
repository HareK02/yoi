完了。

実装内容:
- Decodal dependency/schema を導入し、builtin role Profiles を Decodal sources として追加した。
- `ProfileSourceArchive` tar format と `manifest.json` schema を実装した。
- Backend が profile/role selector から archive を構築し、import closure / source digest / archive digest / size/count/depth/path safety を検証するようにした。
- Runtime が `ProfileSourceArchiveRef` / config bundle から archive を prefetch / verify / cache / reuse できるようにした。
- Runtime `ArchiveSourceLoader` は archive-contained Decodal sources のみを解決し、undeclared import / filesystem fallback を拒否するようにした。
- Worker creation の通常 Browser/Backend path は archive-resolved Decodal config を使い、Runtime-local filesystem profile discovery を使わないようにした。
- Worker metadata/config bundle summary に archive id / source graph summary を残し、Browser-facing response には archive content / digest / store path / raw path を出さないようにした。
- Lua filesystem Profile path は compatibility/debug fallback として隔離した。
- Unknown Builtin / Named selectors と missing entrypoint は default fallback せず typed diagnostics で reject するようにした。
- Builtin `.dcdl` は real Backend-built `ProfileSourceArchive` + Decodal + `ProfileConfig` path で resolve できるように修正した。
- Focused worker-runtime / workspace-server tests を追加した。

主な commit / merge:
- implementation: `a823d414 feat: add decodal profile archives`
- review fix: `a2833dad fix: reject unknown profile archive selectors`
- merge into orchestration: `0334c572 merge: decodal profile archives`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（41 lib tests + 5 main tests + doc tests）
- `cargo test -p yoi-workspace-server`: pass（69 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass

補足:
- implementation branch merge 時に Ticket record conflict が発生したため、Orchestrator 側の Ticket item/thread を保持して解決した。product/code/archive changes は merge 済み。