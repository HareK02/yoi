# Pod 間コールバック通知

## 背景

spawned Pod がイベント（ターン完了、エラー、終了、scope 又貸し）を発生させたとき、spawner に自動通知する仕組みが必要。`Method::Notify`（実装済み）が受け口となり、spawned Pod がそこにコールバックを送る。

## 依存

- `tickets/spawn-pod-tool.md`: spawn 記録と callback address の受け渡し
- `Method::Notify`: 実装済み。コールバック受信 → LLM への system message 注入

## 仕組み

### callback address

spawn 時に spawner が自身の callback address を spawned Pod に渡す:
- ローカル: spawner の unix socket path
- リモート: `insomnia@host:pod-name` 形式（将来。`docs/network-peering.md` 参照）

spawned Pod はこの address を保持し、イベント発生時に一発接続して通知を送り、即切断する（webhook モデル）。

### 通知の種類

| 通知 | タイミング | 含まれるデータ |
|---|---|---|
| ターン完了 | spawned Pod の1ターンが終了 | name |
| エラー | spawned Pod でエラー発生 | name, error_message |
| 終了 | spawned Pod が停止 | name（scope 返却のトリガー） |
| scope 又貸し | spawned Pod が自身の scope を別 Pod に委譲 | name, sub_name, sub_pod_socket, delegated_scope |

### 通知は「シグナル」のみ

通知にはイベント種類と最小限のメタデータだけを含める。応答テキスト全体は含まない。spawner が内容を知りたければ `ReadPodOutput` で取りに行く。

### spawner 側の処理

1. callback を受信
2. `pod.push_notification(source, formatted_message)` でバッファに追加
3. `PodInterceptor::pre_llm_request` が次の LLM リクエスト時に context に注入
4. IDLE なら `Method::Notify` の IDLE パスで自動ターン起動

### scope 又貸し通知の特殊性

spawned Pod (B) が孫 Pod (D) に scope を又貸しした場合:
- B は spawner (A) に通知: 「`/src/core` を D に委譲した。D の socket は ...」
- A は D の存在と address をこの通知で知る（= 親からの紹介）
- A の spawn 記録に D が追加される
- B が死亡しても A は D を直接把握しており、scope lock file の stale 回収で `delegated_from` が A に付け替わる

### コールバック送信の実装箇所

spawned Pod 側に callback 送信の仕組みが必要:
- **ターン完了**: Controller の `RunEnd` イベント発行時に callback を fire
- **エラー**: Controller の `Error` イベント発行時に callback を fire
- **終了**: Controller の shutdown シーケンス内で callback を fire
- **scope 又貸し**: `SpawnPod` ツールの実行後に callback を fire

### ポーリングでの代替

コールバックが失敗しても、spawner は `ListPods` のポーリングで状態を拾える。コールバックは最適化であり、唯一の手段ではない。

## 設計で決めること

- **callback の message format**: JSON? protocol crate の型を再利用? 独自の軽量フォーマット?
- **callback 送信の非同期性**: ターン完了時に callback 送信を await するか、fire-and-forget で spawn するか
- **callback 失敗時のリトライ**: リトライするか、ログだけ出して諦めるか
- **callback address の更新**: spawner が再起動して socket path が変わった場合の再登録手順

## 完了条件

- spawned Pod のターン完了・エラー・終了時に spawner の callback address に通知が送信される
- scope 又貸し時に spawner に通知が送信され、spawner が孫 Pod の存在を把握する
- callback 受信が `Method::Notify` パイプラインに乗り、spawner の LLM に system message として伝わる
- callback 送信が失敗しても spawned Pod は続行する
- 単体テストで各通知種別の送受信が検証される

## 範囲外

- リモートの callback（SSH 越し）。ローカル unix socket のみ
- callback の配信保証（at-least-once / exactly-once）。fire-and-forget で十分
