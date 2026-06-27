embedded `worker-runtime` を既存 `worker` crate の実行 lifecycle に接続する adapter を実装し、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `worker` crate に `runtime-adapter` feature と `WorkerRuntimeExecutionBackend` adapter を追加。
- `worker-runtime` は `worker` に依存しない下位境界のまま維持し、crate dependency cycle を回避。
- embedded Runtime 作成時に `workspace-server` が adapter を install。
- Backend Worker input API から embedded Runtime execution backend に委譲し、既存 Worker run lifecycle (`Method::Run`) に接続。
- Worker の `protocol::Event` を Runtime observation bus / transcript projection に bridge。
- builtin profile selector double-prefix を修正。
- run_state/status projection を実 Worker lifecycle event に合わせて更新。
- spawn execution failure は Browser-facing API で sanitized `Rejected` diagnostic として扱い、failed/unconnected 状態を input-capable に見せない。
- fake/providerless/canned assistant response は導入していない。
- Browser-facing API に raw worker handle / execution handle / socket / session path / credential / secret ref / raw manifest path を出していない。
- `can_stream_events` / `can_read_bounded_transcript` public capability は復活させていない。

Integrated commits:
- `18526ee36264610048f48b07b5db50ce86852fd2 feat: connect runtime worker execution adapter`
- `9069b035041d17e7c52a454a7563cc5f0b7e1f61 fix: connect embedded runtime input lifecycle`
- `7e29ff5ec99dcc748fa5a511cda5bae31fec124b fix: reject embedded spawn execution failures`
- merge: `c3ed223d merge: worker runtime worker adapter`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p worker --features runtime-adapter runtime_adapter`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Known non-blocking warning:
- `cargo test -p yoi-workspace-server` reports existing warning `field next_sequence is never read` in `crates/workspace-server/src/companion.rs`.

Cleanup は child implementation worktree / branch と related role Pods のみを対象に実施する。