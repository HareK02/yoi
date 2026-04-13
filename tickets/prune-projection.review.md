# prune-projection レビュー

## 要件の充足

チケットが定義した3つの設計方針は全て達成されている:

1. **判定結果を index 集合として保持** — `prunable_indices` がそのまま該当
2. **PreLlmRequest hook でコンテキスト構築時に content を省く** —
   `PruneHook::call` で `context` に対してのみ書き換え
3. **Worker の history を一切変更しない** — `worker.rs:701` の
   `let mut request_context = self.history.clone()` により、hook に渡るのは
   clone。`build_request` で使われた後に破棄され、`self.history` には戻らない

## アーキテクチャ

責務分離が綺麗に実装されている:

- `llm-worker::prune` には pure な `prunable_indices` と `PruneConfig` のみ。
  mutation 系 API (`apply_prune`, `PruneResult`) は完全に削除
- `PruneHook` 内に content ドロップの inline ループ。意図的にインライン化されており、
  「射影は呼び出し側の責任」という設計が形になっている
- `PruneHook` の doc comment で `worker.rs:701` への参照により
  「なぜ mutate しても安全か」を明示

## 指摘と対処

### 1. clone 依存の脆さ（非ブロッカー、未対処）

現在の安全性は `worker.rs:701` の clone に依存している。worker の実装変更で
clone が消えると設計が即座に壊れる。対策案:

- worker.rs の clone 箇所に「この clone は射影 hook に依存されている」旨の
  コメントを足す（軽い対処）
- `PreLlmRequest` hook の型を `RequestContext` のような専用型にして
  型レベルで分離（本格対処）

将来的な事故防止の観点で気になるが、チケット範疇の外。

### 2. PruneHook の射影ロジックに単体テストが無い（非ブロッカー、未対処）

`llm-worker::prune` の `apply_prune` テストを削除したのは正しいが、
`PruneHook::call` の射影ロジックには単体テストがない。
`PruneHook::call` を呼んで、context が変更され、かつ元 history が
変わらないことを検証する統合テストがあると安心。

## 判定

承認。
