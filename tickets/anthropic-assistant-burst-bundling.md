# Anthropic projection: assistant ターン内ブロックを 1 message に束ねる

## 背景

`crates/llm-worker/src/llm_client/scheme/anthropic/request.rs` の `convert_items_to_messages` は、Worker が 1 ターンで生成する `[Reasoning, assistant_message, ToolCall]` の連列を、Anthropic wire 上で **複数の隣接した assistant message** に分割している。

具体的には:
- `Item::Reasoning` を `pending_assistant` に push
- 次の `Item::Message { Role::Assistant }` が到来すると `pending_assistant` を flush し、自分自身は別 message として messages に直 push
- 続く `Item::ToolCall` は再び `pending_assistant` に積まれ、turn 末で flush され 3 つ目の assistant message に

結果として 1 turn が `assistant[Thinking] / assistant[text] / assistant[tool_use]` の 3 message に展開される。

Anthropic Messages API は user/assistant の交互を要求し、同一論理 turn 内の thinking/text/tool_use は **1 つの assistant message の `content` 配列** に並べる仕様。新世代 Claude (Opus 4.5+/Sonnet 4.6+) で thinking signature を round-trip する際、隣接 assistant message に分かれていると signature の文脈が崩れて 400 になる懸念がある（reasoning-history-persist のレビュー指摘）。

なお、本バグは reasoning-history-persist で導入されたものではなく、`assistant_message` + `tool_call` の組合せで以前から存在していた pre-existing な分割。Reasoning が同じ flush 経路を継承した形。

## 要件

- 同一論理ターンに属する `Item::Reasoning` / `Item::Message(Assistant)` / `Item::ToolCall` を、Anthropic wire 上の **1 つの assistant message の `content` 配列** に束ねる
- 順序は arrival 順 (= history 順)。Anthropic 仕様の典型は thinking → text → tool_use
- user / system role の `Item::Message` や `Item::ToolResult` を境界として assistant burst を区切る
- 既存の breakpoint (cache_control) 計算が壊れないこと: 各 item のオリジン index → (msg_idx, part_idx) マッピングは flush_pending 経由で記録されているので、Item::Message(Assistant) も pending を経由するように揃えれば自然に追従する
- Single-text 専用の `AnthropicContent::Text` shorthand は assistant burst 内 1 part のみのときに限定して維持するか、簡潔さのために常に `Parts` 形式に統一するかは実装時に判断
- 既存テスト群（`completed_turn`, `single_text_message_uses_text_shorthand_without_breakpoint`, `breakpoint_on_tool_result_head` 等）の意図を逸脱しないよう更新

## スコープ外

- モデル世代別の thinking keep/strip デフォルト分岐（reasoning-history-persist のフォローアップ候補と同じ扱い）
- `clear_thinking_20251015` context-edit
- prune.rs の reasoning aware 化
