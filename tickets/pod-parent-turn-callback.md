# 親が投げたターンのみを子→親 callback の対象にする

## 背景

子 Pod は `controller.rs` の `run_with_cancel_support` で turn 終了時に `PodEvent::TurnEnded` / `PodEvent::Errored` を `parent_socket` 宛に fire-and-forget している。現状この発火条件は「`pod_future` が `Finished` / 失敗で抜けたら」であり、その turn が何起点かを区別していない。

子 Pod のターンは複数の経路で開始しうる:

- 親からの `Method::Run`（`SpawnPod` の初回起動 / `SendToPod`）
- 子内部での `Method::Notify` 起点の auto-kick（`run_for_notification`）。Notify の出どころは孫 Pod からの `PodEvent`、system reminder、外部 Notify など多岐にわたる
- 将来的に増える可能性のある自走経路

入れ子 Pod を本格的に使い始めると、子の Notify 起点 turn が頻発する。これらが完了するたびに親へ `TurnEnded` が飛ぶと、親の `NotifyBuffer` には「自分が投げていないターンの完了」が積まれ、`pending_history_appends` で history に system message として commit され、親まで auto-kick される。親の history は「自分が把握していない子の内部活動」のノイズで埋まり、auto-kick の連鎖も意味的に正当化しづらい。

子→親の callback は、親が投げた仕事の消化境界を伝えるための信号に絞りたい。

## 要件

- 子 Pod が parent へ送る `PodEvent::TurnEnded` は、親由来の `Method::Run` を起点とする turn が `Finished` で完了した場合に限る。
  - 「親由来」の判定は「`Method::Run` で開始された turn」とする。SpawnPod 初回起動 / SendToPod はどちらも `Method::Run` 経由なので両方対象になる。
  - `run_for_notification` 起点の turn は完了しても `TurnEnded` を上げない。
  - `Method::Resume` 起点の turn は親由来として扱う（親が再開を指示した turn のため）。
- `PodEvent::Errored` も同じスコープに揃える。Notify 起点 turn の worker 失敗は `parent_socket` への報告対象外とする（親が知らない指示の失敗報告になるため）。`Event::Error` でローカルに通知される経路は維持。
- `PodEvent::ShutDown` は turn とは独立した Pod プロセス終了通知なので、本チケットの対象外（従来通り常に発火）。
- `ScopeSubDelegated` も turn とは独立しているので対象外。
- 親側の受信処理（`apply_event_side_effects` / `NotifyBuffer` への投入 / history への append）は変更しない。発火源を絞ることで自然にノイズが減る前提。

## 完了条件

- 子 Pod を spawn して `SpawnPod` の初回 Run または `SendToPod` で投げた turn が `Finished` で完了すると、親の history に当該 `TurnEnded` 由来の system message が 1 件 append される。
- 子 Pod が孫 Pod からの `PodEvent::TurnEnded`（または他の Notify）で auto-kick された turn が完了しても、親の history には何も append されない。
- 親由来 turn が worker エラーで失敗すると親に `Errored` が届く。Notify 起点 turn の worker エラーは親に届かない。
- ユニットテストで「`Method::Run` 完了 → 親に届く」「`run_for_notification` 完了 → 親に届かない」「`Method::Resume` 完了 → 親に届く」「Notify 起点 turn の Errored → 親に届かない」を最低限カバーする。

## 範囲外

- Pod プロセス自体の永続化/復元（別途検討）
- `ShutDown` / `ScopeSubDelegated` の発火条件変更
- 親が投げた個々の Run を ID で識別して相関させる仕組み（現状は「届いた / 届かない」で十分なので導入しない）
