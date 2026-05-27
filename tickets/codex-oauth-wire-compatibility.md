# CodexOAuth: Codex CLI との wire 互換性を揃える

## 背景

CodexOAuth / OpenAI Responses 経路で、tool result 後の次 LLM request が `stream_open_start` のまま戻らず、Pause / Resume または cancel まで進まない症状が複数回観測された。直近の trace では local の prune / request build は完了しており、停止位置は `client.stream(request).await` 内、つまり ChatGPT backend への stream open 前後だった。

同時に、公式 Codex CLI 実装と insomnia の CodexOAuth transport には wire level の差分がある。

- insomnia は streaming request に `Accept: text/event-stream` を明示していない。
- insomnia は `prompt_cache_key` は body に送るが、Codex CLI が送る `session_id` / `x-client-request-id` header を送っていない。
- insomnia は Codex backend 向け request body compression を行っていない。
- insomnia は Codex backend request の stream open / token refresh に hard timeout を持たない。
- 公式 Codex 側には 401 recovery や request telemetry があるが、insomnia 側の CodexOAuth は最小実装に留まっている。

Prune 発火後に prompt cache が効かない cold request になり、巨大 request が非圧縮で ChatGPT backend に送られている可能性がある。まずは timeout / trace の追加調査より前に、Codex CLI と明確に異なる wire behavior を揃える。

## 要件

- CodexOAuth / ChatGPT backend 向け Responses streaming request を Codex CLI の wire behavior に近づける。
  - `Accept: text/event-stream` を送る。
  - stable conversation key がある場合、body の `prompt_cache_key` だけでなく header にも `session_id` と `x-client-request-id` を送る。
  - CodexOAuth request では request body compression を利用する。
- 公式 OpenAI API key 経路や Anthropic / Gemini / Ollama 経路に不要な Codex 専用 header / compression を混ぜない。
- CodexOAuth 判定は llm-worker が provider crate の具象型に依存しない形で行う。
- wire 互換性の regression test を追加する。
  - CodexOAuth 相当の custom auth では streaming request が `Accept: text/event-stream` / `session_id` / `x-client-request-id` / `Content-Encoding: zstd` を送る。
  - non-Codex auth では Codex 専用 header / compression を送らない。
- timeout は全体方針として必要だが、この ticket では Codex CLI との差分解消を優先する。timeout は別 ticket / 後続対応で扱う。
- pre-stream lifecycle trace の追加は、Codex wire behavior を揃えた後も再発する場合に行う。

## 完了条件

- CodexOAuth 相当 request の wire headers / compression が test で確認されている。
- non-Codex request に Codex 専用 wire behavior が漏れない test がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- Prune の閾値調整。
- stream open / request 全体 timeout の設計実装。
- 401 recovery の本格実装。
- Codex WebSocket transport への移行。
- 追加 trace instrumentation。
