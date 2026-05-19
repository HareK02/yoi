# Pod state: SpawnedPodRegistry の永続化と復元

## 背景

`SpawnedPodRegistry` (`crates/pod/src/spawn/registry.rs`) は spawner の子 Pod 一覧を保持しているが、現状 `<runtime_dir>/{pod_name}/spawned_pods.json` への write-through のみで、再起動した spawner が子 Pod 一覧を rebuild する経路が無い（コメントに future work と明記）。

`pod-state-backend` で Pod metadata の永続 backend が用意されたあと、本チケットで spawned children registry も同じ永続層に乗せる。

## 要件

- spawned children registry の永続化:
  - 各 child について最低限: pod name, socket path, delegated scope, callback address。child の session id を含めるかは実装時判断（Pod 名から `pod-name-resume` で引けるなら冗長になる）。
  - 書き込みタイミング: `SpawnPod` / callback 受領 / `StopPod` の各点。runtime dir への write-through と同期して永続層にも書く。
  - 読み込みタイミング: spawner Pod の起動時に Pod state から initial load。
- 復元後の spawner で `ListPods` / `SendToPod` / `StopPod` が機能すること。
  - `ReadPodOutput` の read cursor は **永続化対象外**（session-lifetime / in-memory のままで良い）。
  - 到達不能になっている child（socket が消えている等）は registry から除外しつつ、最低限のログを残す。
- `pod-state-backend` で追加した backend trait を再利用し、専用層を増やさない（同じ Pod state の一部として持つか、隣接 entry として持つかは実装時判断）。

## 完了条件

- spawner Pod を再起動した後、永続化された child 一覧から `ListPods` が復元される。
- 復元された child に対して `SendToPod` / `StopPod` が到達可能なものに限って成功する。
- `cargo check --workspace` および `cargo test -p pod` が通る。
- runtime dir の `spawned_pods.json` は引き続き存在しても良いが、永続正本ではない（消えても永続層から復元できる）ことを test で確認。

## 範囲外

- `ReadPodOutput` cursor の永続化。
- 高度な child Pod 監視 / 自動再起動。
- TUI 上の Pod ツリー UI。

## 関連

- `tickets/pod-state-backend.md` (前提)
- `crates/pod/src/spawn/registry.rs`
- `crates/pod/src/runtime/dir.rs`
