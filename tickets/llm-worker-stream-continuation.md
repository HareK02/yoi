# llm-worker: ストリーム途中失敗時の継続

## 背景

LLM 応答の SSE ストリームを読んでいる途中で upstream が切れると、
`crates/llm-worker/src/llm_client/transport.rs:231` で
`ClientError::Sse(...)`（中身は `eventsource_stream::Error::Transport`、
さらに reqwest の `error decoding response body`）として上に投げられ、
`worker.rs:933 stream_response` が `WorkerError` に変換して Run 全体が中断する。

実例: セッション `019de419-6f8b-71a0-9e56-f0b9a6c7098b.jsonl:236`。
直前まで text / tool_use を Timeline に積み続けていたが、
最後の SSE フレームが届く前に接続が切れ、Run が落ちた。

ここで重要な点:

- 出力トークンは upstream 側で既に発生しており、課金済み。
  単純に同じ request を再送すると二重出力 + 二重課金。
- Anthropic / OpenAI のいずれの API も、途切れた SSE を
  resume するエンドポイントは持たない。継続したい場合は
  「assistant turn として部分出力を history に置いた状態で再リクエスト」
  という形を自作する必要がある。
- 部分出力の質は内容依存:
  - 完成した text ブロックは原則そのまま history に置ける
  - 未完成の通常 text ブロックも、LLM の仕様上は assistant partial として置けば続きを生成させられる
  - tool_use の `input_json` が途中で切れたブロックは破損 JSON で、そのままは置けない
  - reasoning / thinking ブロックも provider 依存の扱いが要る

このため、これは「リトライ」ではなく「継続 (continuation)」。
`history` を編集する責務であり、`transport.rs` には収まらず、
`worker.rs` 層（または上位）の機能になる。
`feedback_llm_worker_scope.md` の方針（llm-worker は低レベル基盤に留める）にも合致する。

なお `worker.rs:973` 付近で部分 `flush_usage()` だけは既に行っており、
半分くらいは継続を意識した作りになっている。あとは
「壊れていないブロックの確定」と「次 call の起動条件」を足す形。

## 決定した方針

stream 開始後に transport / SSE error で落ちた場合、同じ request をそのまま再送しない。
Timeline に積まれた部分生成を安全な範囲で assistant history として確定し、その history を前提に continuation call を起動する。

- 通常 text は partial でも残す。
  - 完成済み text block はそのまま確定する。
  - 未完成 text block も text として確定し、次の LLMCall で続きを生成させる。
- tool_use は壊れていないものだけ残す。
  - 完成済み tool_use は通常通り確定する。
  - 未完成 tool_use / partial JSON は history に入れず破棄する。
  - 破棄した事実は structured diagnostic event として記録する。
- reasoning / thinking block は初期実装では保守的に扱う。
  - provider が history に安全に戻せる完成 block として扱えるものだけ残す。
  - 未完成または復元不能な thinking/reasoning は破棄し、diagnostic event に残す。
- continuation は自動で最大 5 回試す。
  - backoff は attempt ごとに伸ばす。例: 1s, 2s, 4s, 8s, 16s。
  - 5 回失敗したら turn を中断し、通常の失敗として上位へ返す。
- `Cancelled` / `Aborted` / interceptor `Yield` は continuation より優先する。
  - 明示的な user cancel や上位制御を transport error retry で覆さない。
- 明示的な safety / content filter stop reason が provider event として返る場合は retry 対象外にする。
  - transport / SSE error としてしか見えない場合は continuation 対象にする。
  - 同じ箇所で繰り返し切られる場合は最大 5 回で exhausted する。

## 失敗ログ / 統計

この機能は実運用での発生頻度と回復率を見たいので、continuation lifecycle を structured log として残す。
ログは統計・デバッグ用であり、通常の LLM context へ暗黙注入しない。

最低限記録する event:

- `llm_stream_interrupted`
  - provider / model
  - run_id / turn_id 相当
  - original attempt / continuation attempt
  - error kind: `sse_transport`, `sse_parse`, `body_decode`, `unknown` 等
  - error message
  - committed text block count
  - committed partial text の有無
  - discarded partial tool_use count
  - discarded thinking/reasoning count
  - usage flush の有無
- `llm_stream_continuation_started`
  - continuation attempt
  - backoff duration
  - history に確定した block summary
- `llm_stream_continuation_completed`
  - continuation attempt
  - completion reason
- `llm_stream_continuation_failed`
  - continuation attempt
  - error kind / message
- `llm_stream_continuation_exhausted`
  - attempts
  - final reason

未完成 tool_use を破棄した場合は、可能な範囲で以下も残す。

```json
{
  "event": "discarded_partial_tool_use",
  "tool_name": "Bash",
  "partial_input_bytes": 1234,
  "reason": "sse_transport_error"
}
```

## 要件

- ストリーム開始後の transport / SSE error を `worker.rs` 層で捕捉し、continuation 対象か判定する。
  - pre-stream の transient retry とは別枠にする。
  - 同じ request の単純再送はしない。
- Timeline に積まれた安全な block を assistant history として確定する。
  - 完成済み text block を残す。
  - 未完成 text block も残す。
  - 完成済み tool_use を残す。
  - 未完成 tool_use / partial JSON は破棄し、diagnostic event を記録する。
  - 未完成または復元不能な thinking/reasoning は破棄し、diagnostic event を記録する。
- continuation call を最大 5 回まで自動実行する。
  - attempt ごとに backoff を伸ばす。
  - 成功したら通常の LLMCall 完了として扱う。
  - exhausted したら turn を中断する。
- `Cancelled` / `Aborted` / interceptor `Yield` がある場合は continuation しない。
- provider が明示的な safety / content filter stop reason を正常 event として返した場合は continuation しない。
- continuation lifecycle と破棄した partial block の概要を structured log に残す。
  - 統計として provider/model 別の失敗頻度、回復率、partial tool_use 発生有無を後から集計できること。
- continuation のために context へ一時的な system message を暗黙注入しない。
  - もし LLM に中断事実を知らせる必要が出た場合は、history に残る明示 event/message として設計する。
  - 初期実装では壊れた tool_use は LLM に知らせず、ログにだけ残す。

## 完了条件

- stream 途中で transport / SSE error を起こすモック integration test がある。
- text-only partial response では、未完成 text が history に残り、continuation call が続きを生成する。
- partial tool_use response では、壊れた tool_use が history に入らず、discard diagnostic が記録される。
- completed tool_use は破棄されず、通常通り history に残る。
- continuation が最大 5 回で exhausted し、turn が中断される test がある。
- `Cancelled` / `Aborted` / `Yield` が continuation より優先される test がある。
- structured log から interrupted / started / completed / failed / exhausted が確認できる。
- 課金重複が起きないこと（過去ターンの単純再生成ではなく partial assistant history からの continuation であること）が test または手動手順で確認されている。
- `cargo check` / `cargo test` が `llm-worker` で通る。

## 範囲外

- pre-stream の transient リトライ → `llm-worker-transient-retry`
- ストリーム resume API の実装（プロバイダ側に存在しないので不可能）
- 課金額の自動上限制御
- 壊れた partial tool_use を system message 等で LLM に説明して復旧させる高度な戦略
