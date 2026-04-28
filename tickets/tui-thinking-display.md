# Thinking ブロックの TUI 表示

## 背景

Reasoning（extended thinking）系の対応は llm-worker レイヤまで降りている:

- Anthropic の `thinking`、OpenAI Responses の `reasoning_text` / `reasoning_summary_text`、Gemini の thinking はいずれも `DeltaContent::Thinking(String)` に正規化され、Timeline 層には `ThinkingBlockKind` の Start / Delta / Stop が流れている
- history 上は `Item::Reasoning { text, summary, encrypted_content, ... }` として保持され、session-store にも persist されている

ところが上位層への通り道が無い:

- `Worker` の closure 公開 API には `on_thinking_block` が無い（`on_text_block` / `on_tool_use_block` のみ）
- `protocol::Event` に Thinking 系イベントが無い
- pod controller でブリッジしていない
- TUI に Thinking 用ブロックが無い

結果として、provider が thinking 平文を返していても TUI からは「無音で時間が過ぎる」状態になる。実行中であることが見えず、終わった後に「どれくらい考えていたか」も残らない。

provider ごとに本文を流せるかは異なる:

- **Anthropic**: extended thinking は平文で流れる
- **OpenAI Responses**: モデル / 設定によって本文（`reasoning_text`）が流れない場合がある。`reasoning_summary` は流れることがある
- **Gemini**: thinking は流れるが provider 設定依存

「平文があれば流す。無くても thinking 中であることは見せる」というのが基本方針。

## 方針

- llm-worker → protocol → pod controller → TUI に Thinking の通り道を作る
- worker は `DeltaContent::Thinking` 由来の本文をそのまま渡す。本文を出さない provider のときは Delta が来ないだけで Start / Done は届く
- TUI は実行中とその後の両方を残す:
  - **実行中**: `Thinking... (Xs)` ヘッダ + 本文があれば直近 1 行のライブ表示
  - **終了後**: `Thought for Xs` として履歴に残す。`detail` モードでは累積本文を展開
- token 数表記は当面入れない（`UsageEvent` に reasoning 分離が無く、別チケットで `Usage` を拡張するまで保留）

## 要件

### Worker API

- `Worker::on_thinking_block(setup)` を `on_text_block` と対称に追加。setup は per-block で 1 回呼ばれ、`block.on_delta(|text|)` / `block.on_stop(|full_text|)` を登録できる

### Protocol

- `Event::ThinkingStart`、`Event::ThinkingDelta { text }`、`Event::ThinkingDone { text }` を追加（`text` には完成形を載せる、`TextDone` と同じ流儀）
- 本文を返さない provider では Delta が 0 件のまま Start → Done が届く（破綻しない）
- 1 turn に複数の thinking block が来る可能性がある（provider 都合）。各ブロックは独立して扱う

### Pod Controller

- `worker.on_thinking_block` で上記 3 イベントに変換して `event_tx` に流す

### TUI

- 新ブロック種別 `Block::Thinking` を持つ
- 実行中は以下のように表示:
  ```
  Thinking... (10s)
    <累積本文の末尾を 1 行に切り詰めたもの>
  ```
- 終了後は `Thought for 12s` を残す。`detail` モードでは本文をそのまま展開して読める
- `overview` モードは 1 行（例: `Thought for 12s`）
- 経過時間表示のため、Thinking ブロックが実行中の間は再描画が定期的に走る必要がある（粒度は 1Hz 程度で十分）
- ライブ 1 行の選び方は「累積テキストの最後の改行以降を取り、表示幅で切り詰める」を MVP とする
- 同一 turn 内の複数 thinking block は別ブロックとして表示される

### イベント欠落耐性

- `ThinkingDone` が来ないまま `TurnEnd` が来た場合、対応する `Block::Thinking` は経過時間を凍結した状態で履歴に残す。`ToolCall` 側の `Incomplete` と同じ思想

### History 再生

- `Event::History` の `Item::Reasoning { text, ... }` を `Block::Thinking { text, finished: true }` として復元する（経過時間は持たないので `Thought` のみで時間表示は省く）

## 範囲外

- `UsageEvent` の reasoning_tokens 分離。Anthropic は `output_tokens` に thinking を含み、OpenAI Responses は `output_tokens_details.reasoning_tokens` を別途返すが、現状の `UsageEvent` ではそれを分離していない。本チケットでは token 数表示そのものを行わない
- Anthropic Redacted Thinking（暗号化 blob）の表示。現状 plaintext 経路のみ流れる
- Thinking 本文の Markdown レンダリング / シンタックスハイライト
- thinking を context から prune する話（別軸）

## 完了条件

- Anthropic で extended thinking を有効にした session で「Thinking... (Xs) + 本文 1 行」が live で見え、終了後に `Thought for Xs` として履歴に残る
- 本文を流さない provider 設定でも `Thinking...` ヘッダが表示され、終了後に `Thought for ...` が残る
- `detail` モードで thinking 本文が全行展開できる
- 同一 turn に複数 thinking block が来てもそれぞれ独立に表示される
- `Event::History` 再生で過去の thinking が `Block::Thinking { finished: true }` として復元される
- `ThinkingDone` 欠落でも panic せず、Incomplete 相当の表示で残る
- 既存のテキスト / ツール / notification / compact 表示が壊れない

## Review
- 状態: Approve
- レビュー詳細: [./tui-thinking-display.review.md](./tui-thinking-display.review.md)
- 日付: 2026-04-28
