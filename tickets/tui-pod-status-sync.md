# TUI: Pod 状態同期と Ctrl 系操作の安定化

## 背景

TUI 利用中、notice / alert / compact 表示が出た前後で `Ctrl-C` / `Ctrl-X` が効かない、または意図と違う動作をしているように見えるケースがある。

調査上、notice 表示そのものがキー入力を壊している実装は見当たらない。一方で、TUI が持つ `running` / `paused` などのローカル状態と、Pod controller の実状態がズレる経路が複数ある。

代表例:

- `Event::RunEnd` 後、TUI は idle 扱いになるが、Pod controller 側では memory extract / consolidate / post-run compact などの post-run 処理がまだ続いている場合がある。
  - この区間で notice / alert / compact event が表示されると、ユーザーからは「notice が出て idle に見えるのに Ctrl 操作が効かない」ように見える。
- TUI attach 時は `GetHistory` で履歴を復元するが、Pod の現在 status（Running / Paused / Idle）は復元されない。
  - 実際には Pod が Running / Paused でも、TUI 側は初期値の idle として扱い、`Ctrl-C` / `Ctrl-X` の分岐が実状態と合わない可能性がある。
- TUI の terminal event polling は Pod event が頻繁な時に key input が遅延・取りこぼしに見えやすい構造になっている疑いがある。

## 要件

- TUI が表示・操作に使う Pod 状態は、接続直後および実行中を通じて Pod controller の実状態と一致する。
  - attach / resume 直後でも Running / Paused / Idle が正しく表示される。
  - `Ctrl-C` / `Ctrl-X` の分岐が TUI の古い推測状態に依存して誤動作しない。
- `RunEnd` と post-run 処理中の扱いを整理する。
  - Worker turn が終わった状態と、Pod controller が次の method を即時処理できる状態を混同しない。
  - post-run compact / memory 系処理中に notice が出ても、TUI 上の status と操作可能性が矛盾しない。
- TUI の input polling を見直し、Pod event / notice が多い状況でも key input が starvation しない。
- `Ctrl-C` / `Ctrl-X` の UX を明確化する。
  - Running / Paused / Idle / post-run busy それぞれで、Pause / Cancel / Shutdown / TUI exit のどれを送るかを明示する。

## 完了条件

- Running / Paused な Pod に後から TUI attach しても、status 表示と `Ctrl-C` / `Ctrl-X` の挙動が実状態に合っている。
- `RunEnd` 後の post-run 処理中に alert / compact notice が発生しても、TUI が idle と誤表示して操作不能に見える状態にならない。
- Pod event / notice が連続しても、`Ctrl-C` / `Ctrl-X` が遅延・取りこぼしに見えない。
- 上記の状態遷移について、少なくともユニットテストまたは再現手順で確認できる。

## 範囲外

- TUI の入力キューイング（`tickets/tui-input-queue.md`）。本チケットは状態同期と Ctrl 系操作の安定化に限定する。
- native GUI 側の状態管理。
- notice / alert の文言や見た目の全面 redesign。
