Workspace Backend embedded Runtime を memory-backed から fs-store backed `worker-runtime` へ切り替え、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `workspace-server` で `worker-runtime` の `fs-store` feature を有効化。
- `ServerConfig` に `embedded_runtime_store_root` を追加。
- default store root を user data 配下の `workspace-server/<workspace_id>/embedded-runtime` に設定し、project records / `.yoi/tickets` と混ぜない設計にした。
- 通常の `WorkspaceApi::new` / `new_with_execution_backend` 経路を fs-store backed `EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(...)` に切り替え。
- `Runtime::with_fs_store_and_execution_backend(...)` を使う constructor を追加。
- Backend restart 後に Runtime catalog / Worker snapshot / transcript / ConfigBundle store が復元されることを focused tests で確認。
- restart 後の live execution handle は connected として偽装せず、stale/unconnected diagnostic/projection として扱う。
- Browser-facing API に Runtime store root / raw path / execution handle を出さないことを確認。
- production path が memory-backed runtime に戻らないよう通常 constructor を fs-store backed にした。

Integrated commit:
- `736b05c6 feat: persist embedded workspace runtime in fs store`
- merge: `888e7b68 merge: embedded runtime fs-store`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success (`38 passed`)
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child implementation worktree / branch.