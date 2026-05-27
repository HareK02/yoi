# LLM client: API request timeout を stream lifecycle 全体に入れる

## 背景

CodexOAuth / OpenAI Responses 経路で、tool result 後の次 LLM request が `stream_open_start` のまま戻らず、Pause / Resume または cancel まで進まない症状が複数回観測された。trace では local の prune / request build は完了しており、停止位置は `client.stream(request).await` 内だった。

この timeout 不足は CodexOAuth 固有ではなく、`HttpTransport<S>` を使う HTTP schema 全体に共通する。加えて CodexOAuth には OAuth token refresh request にも timeout がない。

現状の retry policy は `client.stream().await` が error を返した後にだけ効く。in-flight request が response headers 前で戻らない場合、retry / UI observability / run lifecycle が進まない。

## 実装結果

対象コミット:

```text
541b542 fix: add llm request lifecycle timeouts
```

実装内容:

- `ClientError::Timeout { phase, timeout }` を追加し、retryable error として扱う。
- `HttpTransport<S>::stream` の HTTP response headers 待ちを `DEFAULT_STREAM_OPEN_TIMEOUT = 30s` で制限する。
  - timeout phase: `stream_open`
  - 全 HTTP schema 共通。
- Worker 側で stream open 成功後、最初の SSE event を `DEFAULT_FIRST_STREAM_EVENT_TIMEOUT = 30s` で probe する。
  - timeout phase: `stream_first_event`
  - first event は消費せず、stream に戻して通常 dispatch に流す。
  - first event 後の通常 stream には timeout をかけない。
- CodexOAuth token refresh request を `DEFAULT_REFRESH_TIMEOUT = 30s` で制限する。
  - timeout は `RefreshTransient` として扱う。
- `tokio` の `time` feature を `llm-worker` / `provider` に追加した。

追加 test:

- `HttpTransport` response timeout が retryable `ClientError::Timeout { phase: "stream_open" }` になる。
- Worker first stream event timeout が retryable `ClientError::Timeout { phase: "stream_first_event" }` になる。
- first event probe が最初の event を失わず replay する。
- `ClientError::Timeout` が retryable と判定される。
- CodexOAuth refresh timeout が transient error になる。

確認済み:

```text
cargo test -p llm-worker --lib
cargo test -p provider --lib
cargo fmt --check
```

## 完了判定

要件の stream open timeout / first SSE event timeout / CodexOAuth refresh timeout は実装済み。timeout 値の manifest configurable 化、追加 trace、Prune 閾値調整、CodexOAuth 401 recovery は範囲外として残す。

## 範囲外

- timeout 値の manifest 設定化。
- pre-stream lifecycle trace の追加。
- Prune 閾値調整。
- CodexOAuth 401 recovery の本格実装。
