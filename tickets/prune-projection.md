# Prune をコンテキスト射影に変更

## 背景

現状の `apply_prune` は Worker が保持する history の `Item::ToolResult { content }`
を直接 `None` に書き換えている。PreLlmRequest hook の `context: &mut Vec<Item>` は
Worker の history そのものなので、prune すると元の content が永久に失われる。

問題:
- session-store に persist される history が prune 済みになり、restore しても content が戻らない
- compact worker が要約を作る際に、本来参照できるはずの content が消えている
- 「どこまで prune したか」という状態が暗黙的（content が None かどうか）

## 方針

永続化された記録（Worker の history / session-store のログ）は変えず、
LLM に送るコンテキストを組み立てる段階で content を省く。

Prune は「どの ToolResult の content を省略するか」を決める射影ロジックであり、
history の変換ではない。

### 設計の方向

1. prune の判定結果を「省略対象の index 集合」として保持する
   （現在の `prunable_indices` がそのまま使える）
2. PreLlmRequest hook でコンテキストを構築する際に、省略対象の
   ToolResult は content を含めずに組み立てる
3. Worker の history は一切変更しない

### 影響範囲

- `crates/llm-worker/src/prune.rs`: `apply_prune` を廃止または射影版に置き換え
- `crates/pod/src/prune_hook.rs`: history を mutate せず、射影したコンテキストを返す
- PreLlmRequest hook の戻り値: 現在 `PreRequestAction::Continue` で history を
  そのまま使う設計。射影したコンテキストを渡す方法の検討が必要

## 依存

- なし

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — compact worker が content を参照する前提
