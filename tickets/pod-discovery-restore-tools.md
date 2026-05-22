# Pod: 過去 Pod の探索と restore ツール

## 背景

Pod state の永続化と `--pod <name>` resume が入ったことで、名前が分かっている Pod は復元できるようになった。一方で、AI / operator が「過去にどんな Pod があったか」「この名前の Pod は復元できるか」「live attach できるのか、restore が必要なのか」を機械的に調べる導線はまだない。

現在の `ListPods` は主に spawner が知っている spawned child の live/runtime registry を見るためのツールであり、永続化された全 Pod の探索や、過去 Pod の restore 導線としては不十分。今後 Pod 単位で作業を再開する運用を成立させるには、Pod state を正本として過去 Pod を列挙・確認・復元できる tool surface が必要。

ただし、ホスト上の全 Pod state がどの Pod / LLM からも見える設計にはしない。Pod 管理 tool は capability / visibility scope を持ち、呼び出し元が見る権限を持つ Pod だけを列挙・操作できる必要がある。

## 要件

- 永続化された Pod state から、可視性 scope 内の既知 Pod 一覧を取得する tool / protocol API を追加する。
  - 実際の Method / tool 名は実装時に確定する。
  - `session-store` の Pod state backend/FsStore を正本にし、runtime dir の `spawned_pods.json` を正本にしない。
  - state が壊れている Pod や active segment 未確定の Pod は、全体失敗ではなく item 単位の状態として返せるようにする。
  - 呼び出し元に可視性がない Pod は列挙結果に含めない。
- Pod 可視性の制御を設計する。
  - 少なくとも「現在の parent が spawn した child」と「明示的に指定された Pod 名」は扱えるようにする。
  - ホスト上の全 Pod を無条件に返す admin/global tool にはしない。
  - visibility の根拠は Pod state / parent-child registry / manifest capability / explicit user selection のいずれかに寄せ、実装時に確定する。
  - 可視でない Pod に対する detail / restore / attach は not visible / forbidden として、state missing とは区別する。
- 一覧 item には最低限以下を含める。
  - `pod_name`
  - active `SessionId` / `SegmentId`（未確定ならその状態）
  - live socket / runtime が到達可能かどうか
  - restore 可能かどうかと、restore に必要な名前
  - spawned children が永続化されている場合は、その概要（件数や reachable 状態。詳細展開は別 API でもよい）
- Pod 名指定で詳細を取得できる API を用意する。
  - active pointer
  - restoreability
  - live attach 可能性
  - spawned child registry の概要
  - 読めない state / 消えた socket / lock 衝突を区別したエラー
- Pod 名指定で restore / attach を開始できる tool 導線を用意する。
  - live socket が到達可能なら attach 相当の扱いにする。
  - 到達不能だが Pod state があるなら既存の `--pod <name>` / `Pod::restore_from_pod_metadata(...)` 経路で restore する。
  - Pod state が存在しない名前を指定した場合に新規 Pod を作るか、明示エラーにするかは API ごとに曖昧にせず決める。探索・復元ツールとしては、意図しない新規作成を避けるため default はエラー寄りが望ましい。
- 既存の `--pod <name>` / `--session <UUID>` / spawned child 向け `ListPods` / `SendToPod` / `StopPod` と責務を混ぜない。
  - `ListPods` は現在接続中の spawned child registry を見る用途として維持してよい。
  - 過去 Pod の探索 API は Pod state を正本にする。
- live writer 二重起動防止、scope delegation、session lock の責務は既存 registry / lock に任せ、Pod state に lock 責務を追加しない。
- tool result として LLM に返す情報は通常の tool call 履歴に残る形にし、history に残らない context 差し込みで実現しない。

## 完了条件

- 永続化済み Pod のうち、呼び出し元の可視性 scope 内にあるものだけを Pod state から列挙できる。
- runtime dir の `spawned_pods.json` が存在しない状態でも、Pod state から可視 Pod を探索できる。
- Pod 名指定で詳細を取得し、live attach 可能 / restore 可能 / state 不在 / state 破損 / lock 衝突を区別できる。
- Pod 名指定の restore / attach tool が、到達可能 live Pod には attach し、到達不能だが state がある Pod には既存 restore 経路で復元できる。
- 既存の `ListPods` / `SendToPod` / `StopPod` / `--pod` / `--session` の挙動を壊さない。
- unit / integration test で以下を確認する。
  - 複数 Pod metadata の列挙（可視 Pod のみ）
  - 可視でない Pod が列挙されず、detail / restore / attach でも state missing と区別されること
  - active segment 未確定 Pod の表示
  - runtime file が消えても Pod state から探索できる
  - socket 到達可否の反映
  - restore / attach の分岐
  - lock 衝突時に二重 writer を起動しない
- `cargo fmt --check`
- `cargo check --workspace`
- 関連 crate の tests（少なくとも `cargo test -p pod`。tool surface を置く crate に応じて追加）

## 範囲外

- 過去 Pod 一覧の本格 UI / picker（TUI 側の Pod picker は `tickets/tui-pod-restore-picker.md` で扱う）
- fork tree の可視化
- transcript 全文検索 / semantic search
- Pod の自動再起動
- 古い Pod state の GC / retention policy
- session / segment 単位の新しい resume 引数

## 依存 / 関連

- Pod state backend / FsStore 実装
- Pod lifecycle write-through
- Pod 名単位の resume / attach 導線
- SpawnedPodRegistry の永続化と復元
