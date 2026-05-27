# LLM client: API request timeout を stream lifecycle 全体に入れる

## 背景

CodexOAuth / OpenAI Responses 経路で、tool result 後の次 LLM request が `stream_open_start` のまま戻らず、Pause / Resume または cancel まで進まない症状が複数回観測された。trace では local の prune / request build は完了しており、停止位置は `client.stream(request).await` 内だった。

この timeout 不足は CodexOAuth 固有ではなく、`HttpTransport<S>` を使う HTTP schema 全体に共通する。加えて CodexOAuth には OAuth token refresh request にも timeout がない。

現状の retry policy は `client.stream().await` が error を返した後にだけ効く。in-flight request が response headers 前で戻らない場合、retry / UI observability / run lifecycle が進まない。

## 要件

- 全 HTTP schema 共通で、stream open に hard timeout を設ける。
  - request build 後、HTTP request を送り response headers を受け取るまでを対象にする。
  - timeout は retryable error として Worker retry policy に渡る。
  - stream が始まった後の長い正常出力を全体 timeout で殺さない。
- 全 HTTP schema 共通で、first SSE event に hard timeout を設ける。
  - response headers は返ったが最初の stream event が来ない blackhole を検出する。
  - timeout は retryable error として扱う。
  - first event 後の通常 stream にはこの timeout を適用しない。
- CodexOAuth token refresh request に timeout を設ける。
  - auth refresh が戻らない場合も run が無期限に止まらない。
  - refresh timeout は一時的な auth transport failure として surfacing する。
- timeout 値はまず固定 default でよい。
  - manifest configurable 化は後続で必要になった時に行う。
- timeout 発生時の error message は phase が分かるものにする。
  - 例: `stream_open`, `stream_first_event`, `codex_oauth_refresh`。
- 既存の provider-specific SSE parsing / long streaming behavior を壊さない。

## 完了条件

- `HttpTransport<S>::stream` の response headers 待ちが timeout で戻る test がある。
- Worker の first stream event 待ちが timeout で戻る test がある。
- timeout error が retryable と判定される test がある。
- CodexOAuth refresh request timeout の実装または focused test がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- timeout 値の manifest 設定化。
- pre-stream lifecycle trace の追加。
- Prune 閾値調整。
- CodexOAuth 401 recovery の本格実装。
