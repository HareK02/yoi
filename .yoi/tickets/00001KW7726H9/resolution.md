Runtime Worker 起動経路を canonical `ConfigBundleRef + ExecutionBackend` 経路へ一本化し、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- Runtime `CreateWorkerRequest` を `config_bundle_ref` と user-only initial input 中心の内部作成契約へ整理。
- Browser-facing launch semantics と Runtime worker creation request を分離。
- raw workspace / cwd / tool scope / config store / secret / socket / path 類を Runtime create request に含めない境界にした。
- ConfigBundle missing / digest mismatch / profile mismatch を typed error / diagnostic として扱う。
- execution backend 未接続では Worker 作成を拒否し、input-capable Worker が backend 未接続になる経路を塞いだ。
- create 成功時のみ catalog / transcript / event を永続化し、spawn / initial input dispatch rejection は rollback。
- execution binding identity を raw handle / secret / path / socket を含まない non-authority projection として永続化。
- restore 後に live handle がない persisted execution mapping は `stale` として diagnostic `worker_execution_mapping_stale` を出す。
- System initial input を Runtime boundary で拒否し、launch/create が system transcript を注入できないようにした。
- Workspace embedded / Companion / remote-facing creation を canonical Runtime create path に寄せた。
- Remote Runtime projection でも execution status を見て `can_accept_input` / `can_stop` を計算し、stale / unconnected / rejected / errored Workers を input-capable として出さないようにした。
- fake/providerless response bypass は導入していない。

Integrated commits:
- `14bb4934a6374eea64591035e5342088ab0ccd09 runtime: unify worker creation path`
- `c29d10b67bfff1f4a7a1b2742ec05fe63b80c054 runtime: persist execution binding projection`
- `ba7f9d2ee83a946820cc234e847b6531b4a141f3 workspace: respect remote execution projection`
- merge: `bdb339fa merge: runtime worker launch unification`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p worker-runtime --features "fs-store ws-server"`: success
- `cargo test -p yoi-workspace-server`: success (`36 passed`)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child implementation worktree / branch.