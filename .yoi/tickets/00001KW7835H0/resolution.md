旧 Pod 関連 crate を削除し、Worker/Runtime/session-store 境界へ整理した実装を reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `crates/pod-registry` と `crates/pod-store` を workspace / package / lock / source から削除。
- `crates/pod` は引き続き absent。
- `pod-store` 相当の Worker metadata persistence を `session-store::worker_metadata` へ移管。
- Worker metadata root を active Worker data root 側へ整理。
- `pod-registry` 相当の scope allocation 実装を `worker::runtime::worker_allocation` へ移管し、Runtime Worker identity / creation / durable persistence authority ではないことを明記。
- active CLI / tool / test artifact output を旧 Pod authority wording から Worker / allocation / metadata wording へ変更。
- TUI / yoi / worker の active old crate dependency を削除。
- E2E fixture の Worker metadata root を active `manifest::paths::data_dir()` default resolution と整合する `$HOME/.yoi/workers` に変更。
- Runtime Worker identity / creation / persistence authority は `worker-runtime` fs-store + execution backend mapping のまま維持。
- old Pod registry/store/socket/name を active authority として残す互換 alias は導入していない。

Integrated commits:
- `17a9488a4aa0a3dc83be2d3360b6ffd8ffcaeb5a refactor: remove old pod crates`
- `c46e880b fix: finish worker wording cleanup`
- `fdd902d5 fix: align e2e worker metadata root`
- merge: `83d433bf merge: old pod crate cleanup`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo test -p worker`: success
- `cargo test -p session-store`: success
- `cargo check -p yoi`: success
- `cargo check -p yoi-e2e`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child implementation worktree / branch.