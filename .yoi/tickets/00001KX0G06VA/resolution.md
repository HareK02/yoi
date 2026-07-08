完了。

実装内容:
- Runtime-to-Backend resource fetch contract を追加した。
- Backend-issued typed resource handle と `BackendResourceBroker` を実装した。
- v0 resource kind `profile_source_archive` を fetch / verify / cache できるようにした。
- embedded direct Runtime path と remote Runtime HTTP path が同じ typed resource contract を使うようにした。
- Runtime は handle に基づき archive を fetch し、digest / max bytes / content type / expiry / binding を検証してから cache/reuse するようにした。
- Backend broker は stored issued handle を authority として扱い、Runtime-provided mutable handle fields による expiry / max_bytes / policy tampering を fail closed するようにした。
- Runtime cache は Backend authorization / expiry / binding を bypass しないようにした。
- Decodal ProfileSourceArchive / config bundle path を resource fetch boundary に接続し、Runtime filesystem discovery を再導入しない形にした。
- Browser-facing summaries は internal resource handle / endpoint / raw path / archive content/digest/store path / token 等を出さないようにした。
- Focused worker-runtime / workspace-server tests を追加した。

主な commit / merge:
- implementation: `57e96d3b feat: add backend resource fetch api`
- review fix: `e716ae44 fix: harden backend resource handles`
- merge into orchestration: `01f94b75 merge: backend resource fetch api`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（45 lib tests + 5 main tests + doc tests）
- `cargo test -p yoi-workspace-server`: pass（77 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass

補足:
- Reviewer の non-blocking follow-up: stored `audit_correlation_id` echo と Runtime-side response `resource_id` check、remote HTTP response-size limiting のさらなる強化余地。
- queued `00001KX0DSMPT` については、resource-fetch API shape が確定したため再 routing 可能。