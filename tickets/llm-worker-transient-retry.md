# llm-worker: HTTP transient リトライ

## 背景

`crates/llm-worker/src/llm_client/transport.rs` はリトライを持たず、
upstream が一時的に不調だったときのエラーがそのまま `WorkerError` に
伝播して Run が中断する。実セッションでも以下が観測されている:

- セッション `019de419-6f8b-71a0-9e56-f0b9a6c7098b.jsonl:85` で
  `API error (status: 503): upstream connect error ... Connection refused`。
  これは `transport.rs:194-216` で `response.status().is_success()` 前に
  返される pre-stream の経路。リクエストはまだ消費されていない。
- Anthropic の `overloaded_error` (529) や Codex backend の 503 も
  同経路で観測される transient な事象。

これらは「ヘッダが返る前の段階」で出るため、SSE を読み始めて
出力トークンを発生させる前であり、素朴な再送でべき等に復旧できる典型ケース。
ストリームが途中で切れた場合のリカバリは別の話（→ `llm-worker-stream-continuation`）。

## 方針

`transport.rs` の HTTP 送信層に transient エラー向けの再送を追加する。
SSE 読み出し開始後 (`response.bytes_stream()` 以降) のエラーは対象外で、
従来どおり `ClientError::Sse` として上に流す。

scheme（OpenAI / Anthropic / Responses 等）に依存しない共通処理として、
すべての client から同じ振る舞いで使える形にする。

## 要件

### リトライ対象

- HTTP ステータス: 408 / 425 / 429 / 500 / 502 / 503 / 504 / 529
- `reqwest::Error::is_connect()` / `is_timeout()` 由来の送信失敗
- それ以外の `ClientError::Api { status }` および `ClientError::Json`、
  ストリーム開始後の `ClientError::Sse` は対象外

判定は `is_retryable(&ClientError) -> bool` を `error.rs` に置いて一箇所に集約する。

### バックオフ

- フルジッタ付き指数（base/cap は実装時に妥当な値で固定。後で manifest 化したくなったら別ticket）
- `Retry-After` ヘッダがあれば指数バックオフを上書きしてその時間待つ
- 上限: 最大試行回数 + 累積タイムアウトの両方を持つ

### ログ

- リトライ発火ごとに `warn!`（ステータス、attempt 番号、次の wait）

### 既存挙動の温存

- ストリーム途中で切れた場合の挙動には手を入れない
  （`transport.rs:231` の `ClientError::Sse` 経路はそのまま）
- 成功時のレイテンシに観測可能なオーバヘッドを足さない

## 完了条件

- `is_retryable` のテーブル駆動 unit test
- 503 / 529 / connect refused をモックした unit test が、
  規定回数までリトライして「最終的に成功」「上限到達でエラー」の両ケースを通る
- `Retry-After: 5` を返すモックでは指数を上書きして 5s 待っている
  （仮想時間で検証）
- mid-stream で `ClientError::Sse` を起こすモックでリトライが発火しない
- `cargo check` / `cargo test` が `llm-worker` で通る

## 範囲外

- mid-stream（SSE 読み中）の継続再開 → `llm-worker-stream-continuation`
- プロバイダ別の細かい retry policy（共通既定で十分）
- リトライ上限値の manifest からの上書き（必要になったら別ticket）

## Review

- 状態: Approve
- レビュー詳細: [./llm-worker-transient-retry.review.md](./llm-worker-transient-retry.review.md)
- 対象 commit: `7183847`
- 日付: 2026-05-04
