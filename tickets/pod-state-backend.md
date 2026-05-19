# Pod state: session-store backend と FsStore 実装

## 背景

Pod 単位のランタイム状態は現状 `<runtime_dir>/{pod_name}/` 配下 (`status.json` / `history.json` / `spawned_pods.json`) に write-through されているのみで、Pod プロセスの寿命を超える復元ソースとして扱えない。

`session-grouping-introduce` で Session（Segment 群の grouping）が導入されたあとは、Pod は **「どの Session の、どの leaf Segment が現在 active か」を指す軽量メタデータ**を持てば良い。会話本文は session log を唯一の正本とし、Pod state は references のみを保持する。

このチケットは Pod metadata の **backend trait と FsStore 実装の追加**だけに範囲を絞る。lifecycle hook の配線や CLI 導線は後続チケットで扱う。

## 要件

- Pod metadata trait を session-store crate に追加（`Store` 拡張 or 隣接 trait / module。実装時に決定）。
- 保持する内容:
  - active `(SessionId, SegmentId)` 参照
  - Pod 名（key）
  - manifest / scope の snapshot 参照（既存 session log の `pod.scope` snapshot と責務を重複させない範囲で。最低限 latest segment への pointer のみで足りる可能性が高い）
- FsStore のデフォルト layout は `<data_dir>/pods/<pod_name>/` 配下に置く。`<runtime_dir>` は引き続き socket / pid / status など一時状態専用。
- write は session-store の他 write と同じ sync API で揃える。
- read は冪等で、Pod state が無ければ `None` を返すだけ（initial Pod 起動時に作成される）。
- `--pod` resume の入口に必要な `read_by_name(pod_name)` API を提供する。

## 完了条件

- Pod metadata trait と FsStore 実装が `session-store` にあり、minimal CRUD が unit test で確認できる。
- `<data_dir>/pods/<pod_name>/` の layout が決まっている。
- ordered session history のような時系列 audit は本チケットには含めない（write point 配線時に必要なら追加）。
- `cargo check --workspace` および `cargo test -p session-store` が通る。

## 範囲外

- Pod lifecycle の write point 配線（別チケット `pod-state-write-points`）。
- Pod 名単位 resume / attach の CLI 導線（別チケット `pod-name-resume`）。
- SpawnedPodRegistry の永続化（別チケット `spawned-registry-persist`）。
- DB backend 実装。

## 関連

- `tickets/session-grouping-introduce.md` (前提)
- `tickets/pod-state-write-points.md` (後続)
- `tickets/pod-name-resume.md` (後続)
- `tickets/spawned-registry-persist.md` (並行可)
- `crates/session-store/`
- `crates/pod/src/runtime/dir.rs`
