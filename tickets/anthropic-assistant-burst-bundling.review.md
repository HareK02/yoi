# Review: Anthropic assistant burst bundling

## 対象

- Ticket: `tickets/anthropic-assistant-burst-bundling.md`
- Branch: `anthropic-assistant-burst-bundling`
- Reviewed commit: `19badfe fix: bundle anthropic assistant bursts`

## 確認内容

- `Item::Reasoning` / `Item::Message(Role::Assistant)` / `Item::ToolCall` が pending assistant parts に積まれ、user/system message または tool result まで 1 つの Anthropic `assistant` message として flush される。
- parts の順序は history arrival 順を維持している。
- `Item::ToolResult` は pending assistant を先に flush してから user message の `tool_result` part として扱われ、assistant burst の境界になっている。
- user/system message も pending assistant / pending user を flush してから user message として出力され、assistant burst の境界になっている。
- breakpoint / `cache_control` の item index → `(msg_idx, part_idx)` mapping は pending parts に origin item index を持たせる既存構造で維持されている。assistant text item も pending 経由になったため、assistant burst 内の text part に marker を付けられる。
- user/system の single-text shorthand は breakpoint が無い場合に維持されている。assistant 側は burst flush によって `Parts` に統一されるが、ticket の許容範囲内。
- protocol / scope / history persistence / prompt context processing には触れていない。

## 追加テスト

- `assistant_burst_bundles_reasoning_text_and_tool_call`
- `tool_result_and_user_messages_bound_assistant_bursts`
- `assistant_message_breakpoint_maps_to_text_part_inside_burst`

## 検証

- `cargo test -p llm-worker` passed
- `cargo fmt --check` failed due existing unrelated rustfmt diffs outside this ticket's changed file.
- `cargo fmt -p llm-worker --check` failed due existing unrelated rustfmt diffs in other llm-worker files; `crates/llm-worker/src/llm_client/scheme/anthropic/request.rs` was not reported.
- `git diff --check` passed.

## 判断

Approve.

Ticket の要件通り、Anthropic wire 上の隣接 assistant message 分割を解消し、thinking / text / tool_use を 1 assistant message の content parts に束ねている。変更は projection とそのテストに限定されており、既存設計を歪める追加抽象や範囲外の reasoning policy 変更は入っていない。
