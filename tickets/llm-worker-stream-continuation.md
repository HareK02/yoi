# llm-worker: LLM request retry / stream continuation を可視化しつつ安全に継続する

## 背景

`Read` などの tool 実行が完了した後、本来は tool result を含めた次の LLM request が走り、assistant 応答が続く。実運用ではこの次 request が provider / upstream gateway から HTTP 504 を返すことがあり、LLM response stream を開く前の transient failure として retry する必要がある。

現在の問題は、retry / backoff 中であることが TUI に表示されず、「tool は終わったのに、その後の LLM 応答がハングした」ように見えることにある。

また、SSE stream が開始した後に途中で切れた場合、単純に同じ request を再送すると既に受け取った assistant 出力を二重生成させる可能性がある。stream 開始後の interruption では、安全に確定できる assistant output だけを history に積み、次 request で続きを生成する必要がある。

この ticket では、以下を一つの機能として扱う。

- HTTP transient retry / backoff の user-visible 化
- stream 開始後 interruption からの安全な continuation
- どちらの場合も TUI に「何を待っているか」を表示する経路

## 設計方針

LLM request が待っている理由は user-visible operational state として扱う。history / LLM context には入れない。

retry / continuation から protocol / TUI に直接依存させず、Worker の lifecycle event として下から上へ typed event を渡す。`HttpTransport` は 1 回の HTTP request と response classification を担当し、retry / backoff / cancellation / TUI 通知は Worker が管理する。

```text
HttpTransport
  -> ClientError { status, retry_after, ... }
  -> Worker retry / continuation state
  -> llm-worker callback
  -> Pod controller bridge
  -> protocol::Event
  -> TUI transient status / actionbar
```

正常系の制御フローは崩さない。過去実装のように continuation のために worker の共通 turn loop を大きく組み替えない。

正常 stream 完了時は、現行の成功経路をそのまま通す。

```text
stream_response
  -> handle_completed_response
  -> execute_and_commit_tools
```

continuation は `Worker::stream_response` の error branch 周辺に閉じ込める。

## Phase 1: HTTP transient retry を TUI に表示する

### 対象

`client.stream(request).await` が SSE stream を返す前に、HTTP response status / connect / timeout などで retryable error になったケース。

例:

- HTTP 504 gateway timeout
- HTTP 429 rate limit
- HTTP 500 / 502 / 503
- connect timeout

### 実装方針

1. `llm_client::client::LlmClient::stream(request)` は単発 request として維持する。
   - 成功時は `ResponseStream` を返す。
   - stream open 前の失敗は `ClientError` として返す。
   - retry observer 付き entrypoint は作らない。
2. `llm_client::transport::HttpTransport::stream` は retry しない。
   - HTTP status / connect / timeout を `ClientError` に分類する。
   - `Retry-After` がある場合は `ClientError` の metadata として保持する。
3. `Worker` に `open_stream_with_retry` 相当の helper を置く。
   - `RetryPolicy` と `is_retryable(&ClientError)` に従って `client.stream(request.clone())` を再試行する。
   - backoff sleep は cancel / abort より低優先にする。
   - sleep 前に `on_llm_retry` callback を発火する。
4. `Pod` の `wire_event_bridges_on_worker` で protocol event に変換する。
5. `TUI` は retry state を transient に表示する。

### protocol event

名称は実装時に最終決定してよいが、意味は以下を満たす typed event にする。

```rust
Event::LlmRetry {
    llm_call: usize,
    failed_attempt: u32,
    max_attempts: u32,
    wait_ms: u64,
    elapsed_ms: u64,
    status: Option<u16>,
    error: String,
}
```

- `failed_attempt` は「直近で失敗した attempt 番号」として扱う。TUI 表示では次に実行される attempt を `failed_attempt + 1` として表示してよい。
- `status` は HTTP status が取れる場合のみ入れる。504 の場合は `Some(504)`。
- `error` は user-visible になり得るので、API key / Authorization header / request body を含めない。
- retry exhausted は既存の final error 経路で表示する。初期実装では sleep 前の retry notice に絞る。

### TUI 表示要件

- ToolResult 表示後、次 LLM request が retry に入った場合、TUI 上で待っている理由が分かること。
- status line または actionbar に少なくとも以下が出ること。
  - retry 中であること
  - HTTP status または error kind
  - attempt / max_attempts
  - 次 retry までの wait 秒数
- 例: `retrying LLM request after HTTP 504 (attempt 2/4 in 1.2s)`
- 表示は transient とする。次の `LlmCallStart` / `TextDelta` / `ToolCallStart` / `RunEnd` / `Status(Idle|Paused)` など、明確な進行イベントで消える。
- `Event::Alert` のように履歴 block として毎回積み上げない。retry が複数回起きると履歴がノイズで埋まるため。

## Phase 2: stream 開始後 interruption から continuation する

### 対象

`client.stream(request).await` が stream を返した後、stream event を読む途中で error / unexpected EOF が起きるケース。

HTTP status 504 のような stream 開始前 error は Phase 1 の retry 表示対象であり、partial history commit はしない。

### 実装方針

差し込み位置は `Worker::stream_response` の error branch に限定する。

- 正常に stream が完了した場合:
  - 現行と同じ `TimelineDispatch` / collector の結果を使う。
  - 現行と同じ assistant history commit 経路を使う。
  - 現行と同じ tool execution 経路を使う。
  - continuation 用の別 collector / 別 result type で正常系を包み直さない。
- stream 開始前の error の場合:
  - HTTP retry / final error の扱いに任せる。
  - partial history commit はしない。
- stream 開始後の error の場合:
  - それまで `TimelineDispatch` が受け取った block のうち、安全に閉じているものだけを partial assistant message として history に commit する。
  - 未完成の text block / reasoning item / tool_use は commit しない。
  - partial commit 後、同じ turn loop 内で次の LLM request を発行する。
  - cancel / abort / user interrupt は continuation より優先する。

### 設計上の制約

- `run_turn_loop` 全体を新しい generation sequence abstraction で置き換えない。
- 正常系の `stream_response -> handle_completed_response -> execute_and_commit_tools` の流れを維持する。
- continuation 用 state は error branch の局所 state として扱う。
- completed tool_use を stream interruption から復元して即 tool execution へ流すような特殊経路は初期実装では入れない。
  - tool_use は protocol 上の境界が壊れると危険なので、stream が最後まで完了した場合だけ通常 collector から実行する。
- continuation の進行も TUI に user-visible event として表示する。
- continuation のための system reminder を history に積まず context だけへ差し込む実装は禁止。

### 推奨する差し込み形

`stream_response` の成功 result は保ちつつ、stream が途中で切れたことだけを表せる型にする。

```rust
enum StreamCompletion {
    Complete,
    Interrupted { reason: String },
}
```

`Complete` は現行成功経路へ進むだけで、成功時の assistant item / tool call を別 result type へ包み直さない。`Interrupted` には continuation notice と partial commit 判断に必要な理由だけを入れる。

`TimelineDispatch` / collector に partial drain API を追加し、途中中断時に安全に history 化できるものだけを取り出す。

- 完了済み text block
- 完了済み reasoning item
- 必要なら完了済み assistant metadata

未完成 block は破棄する。`abort_current_block` 相当の既存処理があるなら、それを明示的に使う。

擬似コード:

```rust
match self.stream_response(request).await? {
    StreamCompletion::Complete => {
        self.handle_completed_response().await?;
        if self.execute_and_commit_tools(...).await? {
            continue;
        }
        break;
    }
    StreamCompletion::Interrupted { reason } => {
        if continuation_budget.exhausted() {
            return Err(...);
        }
        self.commit_partial_assistant(...).await?;
        self.emit_continuation_notice(reason);
        continue;
    }
}
```

この形なら、正常系は completed branch に留まり、途中中断時だけ continuation branch に入る。

## 要件

- HTTP 504 / 429 / 500 など retryable API status で backoff に入る前に retry notice が発火する。
- retry notice は worker callback として Pod 層に届く。
- Pod は retry notice を protocol event として TUI / socket clients に broadcast する。
- TUI は retry 中の状態を user-visible に表示する。
- retry notice は worker history / LLM context / session log の会話 item には入らない。
- API key / Authorization header / request body / tool output full content を retry event に含めない。
- stream が正常完了した場合、既存の成功経路と同じ挙動になる。
- tool call が含まれる正常 response では、既存と同じ collector 由来の tool calls だけが実行される。
- stream 開始前の HTTP error では partial history が増えない。
- stream 開始後に中断した場合、安全に完了済みの assistant item だけを history に commit する。
- 未完成 tool_use は commit / execute しない。
- continuation は最大回数で打ち切る。
- cancel / abort / user interrupt は continuation より優先される。
- continuation 中であることが TUI に表示される。

## 完了条件

- `HttpTransport` の unit test で retryable 504/503 が transport 内部では retry されず、`ClientError` として返る。
- `HttpTransport` の unit test で `Retry-After` が `ClientError` metadata として保持される。
- `Worker` の test で stream open 前の retryable error に対して `on_llm_retry` callback が呼ばれる。
- `protocol::Event` の retry / continuation event の serde roundtrip test がある。
- Pod controller bridge の test、または既存 bridge test への追加で retry / continuation event が流れることを確認する。
- TUI app test で retry / continuation event が transient state を更新し、進行イベントで clear されることを確認する。
- 正常 stream 完了 + text response の既存テストが通る。
- 正常 stream 完了 + tool_use response の既存テストが通る。
- stream 開始前 error で partial history が増えない test がある。
- stream 開始後 interruption で完了済み text だけが history に commit される test がある。
- incomplete tool_use が commit / execute されない test がある。
- continuation 回数上限の test がある。
- `cargo check` と関連 crate の test が通る。

## 範囲外

- retry policy の manifest 設定化。
- retry 回数や timeout の調整。
- provider 504 自体の削減。
- context pruning / compaction threshold の調整。
- E2E 実プロセス test。
