# TUI: Session 直接 resume を廃止し Pod 単位の attach / restore に寄せる

## 背景

Pod state 永続化と `--pod <name>` resume が入ったことで、TUI が `SessionId` / `SegmentId` を直接選んで復元する必要は薄くなった。現行の `tui -r` は session picker を開き、選択した `SegmentId` から `pod --session <UUID>` 相当の resume を行うが、これは Pod 単位の運用とズレる。

今後の TUI は、通常操作では Session を直接触らず、Pod 名 / Pod state を入口にする。

## 方針

- `tui <podname>` を Pod 名指定の主導線にする。
  - 既に同名 Pod が live なら attach する。
  - live でなければ、Pod state があれば restore する。
  - Pod state がなければ、新規 Pod として作成する。
- `tui -r` / `tui --resume` は Session picker ではなく Pod picker にする。
  - 稼働中 Pod と停止済み Pod state を一覧表示する。
  - live Pod は attach、停止済み Pod は restore する。
  - 最終アクティブ時刻の降順で sort する。
- `tui` 引数なしの場合のみ、現行の「Pod 名を入力して新規 spawn」導線を残す。
  - 既存 name dialog は新規作成用として維持する。
- `--session <UUID>` は互換用に残すか、非推奨扱いにする。
  - 少なくとも TUI の通常導線からは外す。
  - 完全削除する場合は別途明示判断する。

## Pod picker の表示要件

各 row には最低限以下を表示する。

- Pod name
- live / stopped / corrupt などの状態
- 最終アクティブ時刻
- active `SessionId` / `SegmentId` の短縮表示（debug 用。主表示は Pod 名）
- preview text（取得できる場合のみ）

最終アクティブ時刻は、Pod state に持つか、active segment log の最新 timestamp から算出する。実装時に既存 metadata との整合を確認して決める。

## 要件

- `tui <podname>` が以下を満たす。
  - live socket reachable なら attach。
  - socket が無い / 到達不能で Pod state があるなら `pod --pod <name>` 経路で restore。
  - Pod state が無ければ同名の新規 Pod を spawn。
- `tui -r` が Pod picker を表示し、選択した Pod に attach / restore できる。
- `tui -r` は Session 一覧ではなく Pod 一覧を表示する。
- `tui` 引数なしは新規 Pod 名入力 dialog のままにする。
- TUI の通常導線では Session / Segment を直接選ばせない。
- 既存 `--pod <name>` / `--session <UUID>` の CLI 互換を壊さない。

## 完了条件

- live Pod を `tui <podname>` で attach できる。
- stopped Pod state を `tui <podname>` で restore できる。
- unknown Pod name を `tui <podname>` で新規作成できる。
- `tui -r` で live / stopped Pod が最終アクティブ時刻順に出る。
- `tui -r` で選択した live Pod に attach できる。
- `tui -r` で選択した stopped Pod を restore できる。
- TUI の picker / spawn tests を更新する。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p tui -p pod -p session-store`

## 範囲外

- LLM tool としての任意 Pod 管理 API。
- Pod state の可視性 / capability 制御。
- fork tree 可視化。
- session / segment 単位の新しい TUI UX。
- transcript 全文検索。

## 関連

- `tickets/pod-discovery-restore-tools.md`
- Pod state backend / lifecycle write-through / Pod 名 resume
