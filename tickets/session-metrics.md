# セッションメトリクス: Extension 経由の汎用計測レーン

## 背景

セッション中の挙動を後から定量評価したい需要が増えている。直近の動機は Prune projection の効果測定（どこでどれくらいの頻度で発火したか、KV キャッシュ無効化のコストに対して回収トークンがどれだけあったか）だが、Compact 起動条件、Hook の実行時間、ツール呼び出しのリトライ回数など、同種の「セッション中に積み上げて後で引きたい」値は今後も発生する。

現状の session-log は `UserInput` / `AssistantItems` / `LlmUsage` といった状態遷移を典型化した variant のみで、ad hoc な計測値の置き場所がない。一方 `LogEntry::Extension { domain, payload }` という汎用エスケープハッチが既に用意されており、ここに乗せれば hash chain・replay 経路を流用できる。

## 方針

- `LogEntry::Extension` の `domain = "metrics"` 名前空間として実装する。session-store の型は触らない
- メトリクス型は `name + dimensions(sparse map) + value(Option) + correlation_id(Option)` 程度の最小スキーマ。値が測れない次元は `None` で明示し、Prometheus 的な厳格な label set は持たない
- 「後から埋まる値」（例: prune 発火直後の LLM 呼び出しで観測される `cache_read_tokens`）は前 entry に書き戻さず、`correlation_id` を共有する別 metric として流す。集計は読み手側で join
- 集計 / 可視化 API は本チケットでは作らない。session-log を読めば取り出せる、までを到達点とする

## 要件

### メトリクス型

- 専用 crate（または既存の適切な配置）に定義。`serde` で JSON ラウンドトリップ可能
- 必須: `name`（`namespace.metric` 形式の文字列、例: `prune.fire`）、`ts`（u64 epoch ms）
- 任意: `dimensions: BTreeMap<String, String>`、`value: Option<f64>`、`correlation_id: Option<String>`
- 「unknown」は対応フィールドを `None` にすることで表現。schema レベルで dimension の網羅性は要求しない

### 書き込み経路

- `Pod` から呼べる薄いヘルパー（例: `Pod::record_metric(&self, metric)`) を session-store に追加し、`LogEntry::Extension { domain: "metrics", payload: serde_json::to_value(metric) }` として append する
- 既存の `append_entry` フローを踏襲し、hash chain に乗る
- 書き込み失敗（store IO エラー）はメトリクス側で握りつぶす。本体処理を阻害しない

### 読み出し経路

- replay 時、`RestoredState.extensions` に `("metrics", payload)` として既に積まれる（既存挙動）
- メトリクスドメイン側で payload を `Vec<Metric>` に fold するヘルパーを提供
- session-store のテストハーネスから「特定セッションの metric 列を取り出す」サンプルが書ける状態にする

### 最初の利用者: Prune projection

本チケットの完了は、最低 1 つの実利用者が乗っていることを条件とする。Prune を最初の利用者として組み込む:

- `pod::compact::prune` の `attach_prune` 経路で、projection 評価のたびに以下を発行
  - 発火時: `name = "prune.fire"`, `dimensions = { border_turn, candidate_count }`, `value = estimated_savings`, `correlation_id = <次の LLM 呼び出しと紐付ける ID>`
  - スキップ時: `name = "prune.skip"`, `dimensions = { reason }`（`below_min_savings` / `no_candidates` 等）
- 直後の LLM リクエストで `LlmUsage` が記録される際、同じ `correlation_id` を持つ補助 metric `prune.post_request` を併発し、`cache_read_tokens` / `cache_write_tokens` を value/dimension として記録
- `correlation_id` の生成・伝搬経路は実装側で決定。既存の request-id 系があれば再利用

### Resume 互換

- 旧セッション（metric entry を持たないログ）の replay は何も変えない。`extensions` に metrics domain が無いだけ
- payload schema が将来変わった場合、deserialize 失敗した metric は無視してよい（fold ヘルパー側で `serde_json::from_value` の Err を skip）

## 完了条件

- メトリクス型と書き込み / 読み出しヘルパーが定義され、unit test がある
- Prune projection から `prune.fire` / `prune.skip` / `prune.post_request` が session-log に乗る
- 既存セッションログの replay が壊れない（後方互換）
- セッションログから prune metric 列を取り出すテストが通る
- correlation_id で prune 発火と直後の LLM 呼び出しの cache 値が join できることを test で示す

## 範囲外

- 集計 / 可視化ツール（CLI / TUI）。後続で別途
- ワークスペースまたぎのメトリクス集約（複数セッション横断分析）
- リアルタイム購読 API（Watcher 経由の stream 配信）
- session-log 以外の sink（jsonl 別系統、外部時系列 DB 等）
- Prune 以外のメトリクス利用者の追加（Compact / Hook 等は別チケット）
- メトリクス保存量の自動圧縮 / 退避

## 参照

- 設計指針: `CLAUDE.md`（最小の構造化 / 概念の追加は不在が問題になってから）
- `crates/session-store/src/session_log.rs`（`LogEntry::Extension` と `RestoredState.extensions` の既存仕様）
- `crates/llm-worker/src/usage_record.rs`、`crates/llm-worker/src/llm_client/event.rs`（cache_read / cache_write の取得経路）
- `crates/pod/src/compact/prune.rs`、`crates/llm-worker/src/prune.rs`（最初の利用者の挿入点）

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./session-metrics.review.md](./session-metrics.review.md)
- 日付: 2026-05-03
