# Review: llm-worker HTTP transient リトライ

対象 commit: `7183847` (`develop` `1451998` から派生)
対象 branch: `llm-worker-transient-retry`

## 前提・要件の確認

### リトライ対象 (要件)
- HTTP ステータス 408 / 425 / 429 / 500 / 502 / 503 / 504 / 529 → `crates/llm-worker/src/llm_client/error.rs:80-89` の `is_retryable` で網羅。
- `reqwest::Error::is_connect()` / `is_timeout()` → 同 `is_retryable` で対応。
- それ以外 (`Json` / `Sse` / `Config` / 上記以外の `Api`) を弾く → 同所で明示的に `false`。
- `is_retryable` を `error.rs` に置く要件 → 充足。
- テーブル駆動 unit test → `error.rs:104-121` (retryable) / `:113-121` (non-retryable) / `:128-134` (Json/Sse/Config) で網羅。

### バックオフ (要件)
- フルジッタ + 指数バックオフ → `crates/llm-worker/src/llm_client/retry.rs:40-47` で `min(base * 2^attempt, cap)` の上限から jitter で抽選。
- `Retry-After` で上書き → `transport.rs:182-188` で取り出し、`:255` で `retry_after.unwrap_or_else(|| policy.backoff(...))` により上書き。
- 上限: 最大試行回数 + 累積タイムアウトの両方 → `transport.rs:251-258`。`max_attempts` (default 4) と `total_timeout` (default 30s) で二段に打ち切り。

### ログ (要件)
- リトライ発火ごとに `warn!` (status / attempt / wait) → `transport.rs:259-264`。`error = %err` で表示するためエラー本体に status が含まれる (`ClientError::Display` の "API error (status: N)")。`attempt = next_attempt` と `wait_ms` も出ており要件通り。

### 既存挙動の温存 (要件)
- mid-stream `ClientError::Sse` 経路 (旧 `transport.rs:231` 相当) は `bytes_stream()` 以降にあり、retry loop の外。`tests/transport_retry_test.rs:196-227` で「mid-stream Sse は再送しない (受信回数 1)」を検証済み。
- 成功パスの計測オーバヘッド: 成功時は loop 1 周のみで `break resp` → 余分な await/allocation なし。`headers.clone()` が増えるが retry 不要時にも走る。`HeaderMap` の clone は軽量だが厳密にはわずかな増分あり。問題視するレベルではない。

### 完了条件
- `is_retryable` のテーブル駆動 unit test → ✓ (`error.rs` mod tests)
- 503 / 529 / connect refused モックでリトライ + 上限到達 → ✓ (`transport_retry_test.rs:88-153`)
- `Retry-After: 5` の指数上書き → 実装は「base/cap=1ms に絞った policy で `Retry-After: 1` を観察し、実時間で elapsed >= 1s を assert」(`transport_retry_test.rs:155-194`)。チケット文面の「5s / 仮想時間」とは異なるが、検証している性質 ("指数バックオフを `Retry-After` 値で上書きして待つ") は同一。後述。
- mid-stream `Sse` で再送しない → ✓ (`:196-227`)
- `cargo check` / `cargo test` → ✓ (実行確認済み: lib 8/8 pass、`transport_retry_test` 6/6 pass、warning は既存の `end_scope` のみで本 PR 由来でない)

## アーキテクチャ・スコープ

- 配置: `llm_client/retry.rs` 新設、`error.rs` への `is_retryable` 追加、`transport.rs` 内のリトライループ。すべて `llm-worker` の低レベル基盤に閉じており、上位層 (worker / pod) を巻き込んでいない。`MEMORY.md:llm-worker scope` に整合。
- scheme 非依存性: `Scheme` trait に retry 関連の API を増やしていない。`HttpTransport<S>` の単一の retry loop で全 scheme をカバーする方針はチケット記載 ("scheme に依存しない共通処理") と一致。
- 依存追加: `wiremock = "0.6.5"` が `[dev-dependencies]` に追加 (`Cargo.toml:28`)。`cargo add` 経由かは履歴から確認できないが、配置・バージョン指定とも妥当で `MEMORY.md:Use cargo add` の趣旨に反する痕跡は見えない。
- API 表面: `HttpTransport::with_retry_policy` を新規 pub 公開。チケット範囲外 (manifest 化) に踏み込んでおらず、test 用 + 将来フックとして 1 メソッドのみ。過剰実装ではない。
- 不要な抽象: なし。`RetryPolicy` は struct + `Default` + `backoff()` のみで、trait 化や builder 化に走っていない。`classify_error_response` は旧 inline ロジックの抽出で、retry loop が再利用するために必要な分解。

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

- **`Retry-After` 検証の方針差**: チケット文面は「5s / 仮想時間」、実装は「1s / 実時間」。検証している不変条件 ("`Retry-After` が指数バックオフより大きいときに `Retry-After` 値に従って待つ") は同等で、テスト所要時間も ~1s なので CI への影響も限定的。ただし将来 `tokio::time::pause()` を使う方針に揃えたくなったら、`reqwest`/`wiremock` を実 socket でなく `tower::Service` レベルに張り替える必要があり、軽い改修で済まない。本 PR の判断は妥当だが、ticket 文面とのズレはレビューレポートに残す価値あり。
- **connect refused テストが retry 回数を assert していない** (`tests/transport_retry_test.rs:136-153`): 最終的に `Http` エラーが返ることだけ確認しているので、`fast_policy(3)` で実際に 3 回試行したかは間接確認のみ。retry loop 自体は wiremock の 503/529 ケースで覆われているので致命傷ではないが、`reqwest::Error::is_connect()` 経路を打つ唯一のテストでもあるので、「connect 失敗が retryable と判定されて確実に retry される」ことを示すために試行回数を観測したい。listener を立てて connection を drop する方式 (`std::net::TcpListener::bind` + `drop`) で観測できる。
- **Retry-After の HTTP-date 非対応**: 仕様上は秒数のみ対応で OK (ユーザー合意済み)。Anthropic / Codex の実装が将来 HTTP-date 形式を返したらサイレントに無視されて指数バックオフに戻る (現状エラーにはならない)。後追いで気付きにくい挙動なので、`Retry-After` 形式の合意を別ドキュメント (この `.review.md` か後続 ticket) で残しておくと安全。
- **Custom auth provider のヘッダ取得が retry loop の外** (`transport.rs:228` の `build_headers().await?`): 初回試行のヘッダを clone して使い回す。OAuth access token の有効期限が極端に短い場合、長め (>1 token TTL) の `total_timeout` で粘ると古いトークンを送り続ける可能性がある。現状の Codex OAuth は実質長寿命で問題ないが、retry 中に provider を再 invoke する設計に切り替えたいときの拡張余地として認識しておくと良い。今 ticket では範囲外。

### Nits

- `jitter_nanos` で `max_nanos + 1` のオーバーフロー: `cap = 10s` 想定なら届かないが、将来 cap を `u64::MAX` に近づけたら破綻する。`max_nanos.saturating_add(1)` または `wrapping_rem` 等への置き換えで防御可。実害なし。
- `tokio::time::Instant` を import しているが、`std::time::Instant` でも実害はない (`tokio::time::sleep` は両方を扱える)。`tokio::time::pause()` 連動を見越した選択であれば、コメントで一行触れておくと意図が伝わりやすい。

## 判断

**Approve** — チケットの背景・要件・完了条件はすべて満たされており、コードベースの層構造・命名・抽象度を歪めていない。`Retry-After` テストの方針差は実装者が自己申告しているとおり等価な性質を検証しているため非ブロッキング。`with_retry_policy` の pub 化と manifest 送りの分離もチケット記載どおりで、過剰実装ではない。
