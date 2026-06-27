`worker-runtime` に Worker execution backend 境界を追加し、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `WorkerExecutionBackend` trait / handle / context / status / result / operation / outcome を追加。
- backend 未接続 Worker への input を typed `WorkerExecutionUnavailable` として拒否。
- backend dispatch の busy / rejected / errored / unsupported を typed rejection として扱う。
- backend から Runtime observation bus へ `protocol::Event` を publish する hook を追加。
- stop / cancel unsupported の typed rejection を追加。
- `WorkerSummary` / `WorkerDetail` に raw handle/path/credential を含まない execution status projection を追加。
- Runtime が fake/providerless assistant response を生成しない境界を維持。
- `can_stream_events` / `can_read_bounded_transcript` public capability は復活させていない。
- `ws-server + fs-store` restore path で observation bus state を正しく初期化。

Integrated commits:
- `2d5971738478f832ba9a135601ea11dda60c565d feat: add worker execution backend boundary`
- `761b60c85750d03c119733a088fb5073f9b37e9a fix: initialize restored worker observations`
- merge: `0753e155 merge: worker runtime execution backend`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Cleanup は child implementation worktree / branch と related role Pods のみを対象に実施する。