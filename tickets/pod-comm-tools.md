# Pod 間通信ツール: SendToPod / ReadPodOutput / StopPod / ListPods

レビュー中: [pod-comm-tools.review.md](pod-comm-tools.review.md)

## 背景

`SpawnPod` で Pod を生成した後、spawner の LLM が spawned Pod に指示を送り、結果を読み、完了したら停止させる手段が必要。

## 依存

- `tickets/spawn-pod-tool.md`: SpawnPod による Pod 生成と spawn 記録

## ツール群

すべて**都度接続の request-response** で動作する。spawned Pod の socket に接続し、操作を行い、切断する。

### `SendToPod`

既知の Pod にメッセージを送る。

入力:
- `name`: 対象の Pod
- `message`: 送信するテキスト

出力:
- 送信確認

内部動作:
- spawn 記録から `name` の socket path を引く
- socket に接続 → `Method::Run { input: message }` を送信 → ack 受信 → 切断

### `ReadPodOutput`

既知の Pod の出力を読む。前回読んだ位置を自動追跡（カーソルベース）。

入力:
- `name`: 対象の Pod

出力:
- 前回読んだ位置以降の assistant テキスト出力
- 現在の到達性（`alive` / `stopped`）

内部動作:
- spawn 記録から socket path を引く
- socket に接続 → `Method::GetHistory` で履歴取得 → 前回カーソル以降の assistant text を抽出 → カーソル更新 → 切断
- 接続できなければ `stopped` として返す

### `StopPod`

既知の Pod を終了させ、譲渡した scope を回収する。

入力:
- `name`: 対象の Pod

出力:
- 終了要求を送った旨
- 回収された scope の要約

内部動作:
- socket に接続 → `Method::Shutdown` 送信（応答は待たない）→ 切断
- scope lock file を flock → 対象の allocation 削除 → spawner の deny を解除 → unlock
- spawn 記録から対象を削除

### `ListPods`

自分が知っている Pod の一覧と状態を返す。

入力: なし

出力:
- 各 Pod の `name`、`name`、`status`、譲渡中の scope 要約、最終応答の要約

内部動作:
- spawn 記録を元にリストを構築
- 各 Pod に都度接続して health check（`Method::GetHistory` でステータス取得、接続できなければ `stopped`）
- stopped な Pod は scope lock file の stale 回収をトリガー

## 設計で決めること

- **ReadPodOutput のカーソル管理**: spawn 記録内に `last_read_hash` を持つか、別の場所で管理するか
- **StopPod と scope 回収の順序**: 先に Shutdown してから lock file を更新するか、lock file を先に更新するか（Shutdown が失敗した場合の整合性）
- **ListPods の health check コスト**: 大量の Pod がいるとき全部に接続するのは重い。キャッシュ戦略

## 完了条件

- `SendToPod` で spawned Pod にメッセージを送り、Pod がそれを処理する
- `ReadPodOutput` で spawned Pod の最新出力を読め、カーソルベースの差分取得が動作する
- `StopPod` で spawned Pod を graceful に停止でき、scope が spawner に返却される
- `ListPods` で既知の Pod の状態を一覧でき、health check が機能する
- 接続できない Pod は `stopped` として扱われ、scope の stale 回収がトリガーされる
- 単体テストで各ツールの正常系・異常系が検証される

## 範囲外

- コールバック通知は `tickets/pod-callback.md`
- Pod ネットワークの GUI / TUI 可視化
- spawner プロセス再起動後の `spawned_pods.json` からの復旧（現状は write-through のみ）
- `ReadPodOutput` カーソルの永続化（インメモリのみ、再起動で 0 に戻る）
- Pod の詳細ステータス（`running` / `idle`）を `Event::History` に含める拡張
