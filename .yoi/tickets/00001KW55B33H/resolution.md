Workspace Companion を embedded Runtime / `worker` runtime adapter 経由の実行可能 Worker として扱う実装を行い、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `/api/companion/messages` の旧 `companion_llm_not_connected` 固定拒否経路を廃止し、通常の Worker runtime input path (`RuntimeRegistry::send_input`) に接続。
- Companion bootstrap を `builtin:companion` profile / Companion config bundle path に変更。
- Companion transcript projection を Runtime transcript 由来に変更。
- Companion status / transport を input-capable なら `connected`、不可なら `not_input_capable` + typed diagnostics に変更。
- `/api/workspace` の `extension_points.companion_console` も Companion の実 status / diagnostics 由来に変更し、stale `not_connected` / disabled note を削除。
- `ExtensionPointState` に `diagnostics` を追加し、Workspace web type も更新。
- Browser-facing `/api/workspace` から raw `local_root` exposure を削除。
- deterministic test backend により Companion bootstrap / input dispatch / assistant output / observation event / transcript projection を Worker execution path 経由で検証。
- fake/providerless/canned assistant response は導入していない。
- Worker Console path は `runtime_id + worker_id` keyed のまま維持。
- public `can_stream_events` / `can_read_bounded_transcript` capability は復活させていない。

Integrated commits:
- `ee25cfbcfd90983a24091ef30c0128d653095003 fix: route workspace companion through worker runtime`
- `3be193223c7efc67636667d1ad526646da81fb63 fix: report companion console runtime status`
- merge: `eb06b8a9 merge: workspace companion llm worker`

Validation:
- `cargo fmt --all --check`: success
- `cd web/workspace && deno task test`: success
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `cargo test -p yoi-workspace-server`: success (`34 passed`)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child implementation worktree / branch.