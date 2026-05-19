# Pod state: lifecycle 各点での write 配線

## 背景

`pod-state-backend` で Pod metadata の永続 backend が用意された。本チケットでは Pod の lifecycle 各点で **active `(SessionId, SegmentId)` の更新を Pod state に write-through する** ことを実装する。

## 要件

- Pod state の write point を以下に配置:
  - Pod 作成時: pod name と allocated `SessionId` を初期化。`SegmentId` は first `SessionStart` materialize で確定するので未確定 marker を許容する。
  - first run で `SessionStart` が materialize された後: active `(SessionId, SegmentId)` を確定。
  - compaction: 新 `SegmentId` に切り替わる (`SessionId` は据え置き)。
  - fork (live auto / 過去): 新 `SegmentId` に切り替わる。
  - resume: 起動時に Pod state から `(SessionId, SegmentId)` を解決し、session log を `restore_from_manifest` 相当の経路で復元する。
- session log の `SessionOrigin` を Pod state 側に重複保持しないこと。compaction / fork の構造的 lineage は session-store 側に正本。
- live writer の二重起動は既存の pod-registry / session lock と同等以上に防止する（Pod state にも lock 役割を持たせるかは実装時に判断、ただし pod-registry の責務を歪めない）。

## 完了条件

- 上記 write point で Pod state が更新され、Pod プロセスを再起動しても Pod 名から active session に復元できる。
- compaction / fork により active segment が変わった場合、Pod state と pod-registry の session id が整合する。
- 既存の `--session <UUID>` resume を壊さない。
- `cargo check --workspace` および `cargo test -p pod` が通る。

## 範囲外

- Pod 名単位 resume の CLI 引数導線（別チケット `pod-name-resume`）。
- spawned children の永続化（別チケット `spawned-registry-persist`）。
- ordered session history の audit（Pod state 側に持たせるか、session log だけで足りるかは本チケットで判断。**持つ必要が無いなら持たないこと**を優先する）。

## 関連

- `tickets/pod-state-backend.md` (前提)
- `tickets/pod-name-resume.md` (後続)
- `crates/pod/src/pod.rs`
- `crates/pod-registry/`
