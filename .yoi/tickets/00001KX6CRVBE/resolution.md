Manual Worker/Workdir delete and cleanup operations を実装・レビュー・merge・検証した。

実装内容:
- Backend Worker registry row の pin/unpin API を追加し、Worker list/detail に `pinned` / `retention_state` を返すようにした。
- Runtime ごとの manual cleanup plan API を追加し、Backend Worker/Workdir/link registry と Runtime observation に基づいて Worker delete / Workdir cleanup/delete candidates を生成するようにした。
- Plan candidate は action kind、reason/blocking reason、linked Worker/Workdir ids、pinned state、cleanliness/file status、running-link status、safe な reclaim bytes placeholder を含む。
- Manual cleanup execution API は expected plan revision/digest を要求し、stale plan を拒否する。
- Pinned Worker/history delete、running-linked Workdir cleanup、dirty/unknown Workdir の confirmation なし discard を拒否する。
- Removed/missing Workdir registry record delete は安全条件のもとで separate action として扱う。
- Runtime-observed/materialized Workdirs は trusted clean evidence なしに `clean` とせず、`unknown` として扱い、normal clean cleanup ではなく explicit discard confirmation path に乗せる。
- Browser UI に Runtime Workdirs cleanup preview/execution と Worker pin/unpin controls を追加した。
- UI では verified-clean cleanup と dirty/unknown-state discard を区別し、raw Runtime materialized path を表示しない。

Review:
- 初回 review は dirty Workdir safety の実データ経路が不十分として `request_changes`。
- `361569a6 fix: require discard confirmation for unknown workdirs` で observed Workdir を `unknown` 扱いにし、unknown/dirty cleanup に explicit confirmation を要求するよう修正。
- focused re-review は `approve`。
- `estimated_reclaim_bytes: None` は safe size source が未実装のため許容と判断された。

Merge / validation:
- Merge commit: `4970a58c merge: manual worker workdir cleanup`。
- Final validation passed:
  - `git diff --check`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/manual-cleanup-final-validation-1783708592.txt`