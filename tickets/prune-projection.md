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

## 追加作業: Worker への統合

初回レビューで「pure ロジックが `llm-worker::prune` にあるのに適用は pod 側
Hook で行う」という責務の不整合が見つかった。現状:

- `llm-worker::prune` が `PruneConfig` と `prunable_indices` を提供
- `pod::prune_hook::PruneHook` が `PreLlmRequest` hook として射影を実行

Worker 自身が context window 管理の責任を持つべき。`build_request` の直前
（`worker.rs:701` 付近）で射影すれば `PruneHook` は不要になる。

### 変更内容

- `Worker` が `PruneConfig` を直接保持する
  （`Worker::set_prune_config(config)` 等）
- `worker.rs:701` の `request_context = self.history.clone()` の直後に
  Worker 自身が `prunable_indices` で候補を抽出し、`min_savings` を満たせば
  content を射影する
- `min_savings` 判定のためのトークン会計は Worker から外部に問い合わせる。
  `Worker::set_savings_estimator(Box<dyn Fn(&[Item], Range<usize>) -> u64>)`
  のようにコールバックを注入する形にする
  （Worker は usage 履歴を知らないので、計算は pod 側に残す）
- `pod::prune_hook` モジュールを廃止。Pod は `worker.set_prune_config(...)` と
  `worker.set_savings_estimator(...)` を呼ぶだけ

### 影響範囲

- `crates/llm-worker/src/worker.rs`: `PruneConfig` / savings estimator の
  保持、`build_request` 直前での射影適用
- `crates/pod/src/prune_hook.rs`: **削除**
- `crates/pod/src/pod.rs`: `PruneHook` 生成を `worker.set_prune_config` +
  `worker.set_savings_estimator` に置き換え
- `crates/pod/src/lib.rs`: `prune_hook` モジュールの削除

### 得られるもの

- llm-worker 側に prune 関連コードを集約できる
- `PreLlmRequest` hook から prune 固有の mutation が消える
  （現状 hook に渡る `&mut Vec<Item>` はユーザー向け hook が自由に使えるが、
  Worker 内部の prune と同じ経路を共有するのは混乱の元）
- PruneHook の単体テスト不足問題（初回レビュー指摘2）も Worker のテストとして
  自然に書ける

## レビュー状態

Reviewed — [prune-projection.review.md](prune-projection.review.md)
（追加作業「Worker への統合」は未着手）

## 依存

- なし

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — compact worker が content を参照する前提
