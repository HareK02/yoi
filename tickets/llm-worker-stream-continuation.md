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
  - tool_use の `input_json` が途中で切れたブロックは破損 JSON で、そのままは置けない
  - reasoning / thinking ブロックも provider 依存の扱いが要る

このため、これは「リトライ」ではなく「継続 (continuation)」。
`history` を編集する責務であり、`transport.rs` には収まらず、
`worker.rs` 層（または上位）の機能になる。
`feedback_llm_worker_scope.md` の方針（llm-worker は低レベル基盤に留める）にも合致する。

なお `worker.rs:973` 付近で部分 `flush_usage()` だけは既に行っており、
半分くらいは継続を意識した作りになっている。あとは
「壊れていないブロックの確定」と「次ターンの起動条件」を足す形。

## 詰めたい論点（実装前に決める）

このチケットは仕様議論を含む。以下を確定させてから実装に入る。

1. **部分ブロックの取り扱い基準**
   - 完成した text ブロックは確定して history に push するか
   - 未完の tool_use は捨てるのが妥当か、暫定 stop で残すか
   - reasoning / thinking ブロックの扱い（provider 別）

2. **継続の起動方式**
   - 自動的に次ターンを回す（= retry-like 挙動）/ Pause で上位に判断委譲 /
     manifest で切替、のいずれか
   - デフォルトは何か（自動継続は意図しない出費を生む可能性あり）

3. **ループ防止**
   - 同種の transport エラーが連続 N 回起きたら諦める閾値
   - 「ストリーム開始後ほぼ即座に切れる」が連続するパターンの検知

4. **他の中断要因との優先度**
   - `Cancelled` / `Aborted` / interceptor の `Yield` が同時に起きたときの順序

5. **可観測性**
   - 「途中で切れて継続した」事実をセッションログに残す形
   - `ClientError::Sse(String)` を `Sse { kind: Parse | Transport, msg }` に
     分割するかどうか（診断容易性のため）

## 要件（暫定。論点確定後に再記述）

- ストリーム途中で transport 由来のエラーが出た場合、
  `worker.rs` がそれを catch し、Timeline に積まれた完成ブロックだけを
  assistant items として確定する。
- 未完ブロック（特に壊れた tool_use）は破棄するか、
  破棄したことを示す形で履歴に残す（決定は論点 1）。
- 継続するか中断するかの判定が、論点 2 の決定に従って分岐する。
- 連続失敗時に止まる（論点 3）。
- 既存の `Cancelled` / `Aborted` パスが優先される（論点 4）。

## 完了条件

- 上記論点が決まり、ドキュメント or チケット本文に反映されている。
- ストリーム途中で切れるモックを使った integration test が、
  決まった仕様どおりに継続 or 中断する。
- 課金重複が起きないこと（自動継続でも、過去ターンの再生成は発生しない）が
  test または手動手順で確認されている。
- `cargo check` / `cargo test` が `llm-worker` で通る。

## 範囲外

- pre-stream の transient リトライ → `llm-worker-transient-retry`
- ストリーム resume API の実装（プロバイダ側に存在しないので不可能）
- 課金額の自動上限制御
