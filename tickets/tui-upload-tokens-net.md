# TUI: ステータスライン `↑` をキャッシュ控除後の純アップロード量にする

## 背景

TUI ステータスラインと `Block::TurnStats` で表示している `↑{input}/↓{output}` の `↑` が、tool ループの長いターンで現実離れした大きな値になる。

原因は2つ重なっている:

1. **累積範囲**: `crates/tui/src/app.rs:469-470` で `Event::Usage` ごとに `run_input_tokens += input_tokens` を加算し、`RunEnd` で 0 リセット。1ターン内の全リクエストの input_tokens を単純合計している。
2. **`input_tokens` の規約**: `crates/llm-worker/src/llm_client/event.rs:61-66` のコメント通り、`input_tokens` は「送信した prompt prefix 全体（cache_read + cache_creation 込み）」の占有量で、Anthropic scheme でも `convert_usage` で cache 込みに正規化済み。tool ループでは各リクエストが直前までの history 全体を prefix として送る → そのほとんどが cache hit → cache_read 分が毎リクエスト重複加算される。

結果として `↑` が「このターンで新たにアップロードしたトークン数」として読めず、ユーザー体験が悪い。

## 方針

**ターン内累積は維持**しつつ、各リクエストの加算値を `input_tokens - cache_read_input_tokens`（= cache_creation + 非キャッシュ入力 = 「このリクエストで full price で送った分」）に変える。

そのために protocol-level `Event::Usage` に cache 内訳を追加し、pod が worker UsageEvent から passthrough する。llm-worker 内部 (`UsageEvent`) と session-store (`LogEntry::LlmUsage`) には既に cache_read / cache_creation が載っているので、protocol の穴埋めだけで済む。

cache_creation は「このリクエストで新たにキャッシュに書いた分」で実質アップロードに含めて良い扱いとする（料金的にも full price 側）。よって表示式は `input_tokens - cache_read_input_tokens`。

## 要件

- protocol `Event::Usage` (`crates/protocol/src/lib.rs:284`) に `cache_read_input_tokens: Option<u64>`（最低限これ。cache_creation も載せるかは実装時に判断）を追加。
- pod controller (`crates/pod/src/controller.rs:228-233`) が worker の `UsageEvent` から cache_read を passthrough する。
- TUI 側 (`app.rs:469`, `block.rs:44-48`, `ui.rs:407-422 / 778-793`) は `input_tokens - cache_read_input_tokens` を累積・表示する。`Block::TurnStats` のフィールド意味も同方向に揃える（純アップロード量）。
- `pod/examples/pod_protocol.rs:76` のサンプルプリントも整合させる。
- 既存テスト・session-store 側ログ（`LogEntry::LlmUsage` は cache 内訳を既に持っているので変更不要）が壊れないこと。

## 完了条件

- TUI で tool ループの長いターンを回しても、`↑` が同じ history を毎回再カウントせず、現実的なオーダーに収まる。
- `Block::TurnStats` の `↑` も同じ意味（純アップロード）で揃う。
- protocol / pod / TUI が cache_read 情報を受け渡せる経路ができている。

## 範囲外

- TUI 以外のクライアント（pod_cli 等）の表示変更。
- 料金計算・課金表示の追加（あくまで「アップロード量」の意味付け修正）。
- セッション全体（複数ターン）の累積表示。
- cache hit 率のような追加指標の表示。

## Review
- 状態: Approve
- レビュー詳細: [./tui-upload-tokens-net.review.md](./tui-upload-tokens-net.review.md)
- 日付: 2026-05-03
