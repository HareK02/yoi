# Paused→Run の interrupt 前処理を Pod 内部に閉じる

## 背景

`controller_loop` は `Method::Run { input }` を受けたとき、Pod の status を見て次のように 2 分岐している (`crates/pod/src/controller.rs:629`):

```rust
pending = Some(if status_before == PodStatus::Paused {
    PendingRun::InterruptAndRun(input)
} else {
    PendingRun::Run(input)
});
```

そして `drive_turn` 側でも `pod.run(input)` と `pod.interrupt_and_run(input)` を呼び分けている。

`pod.interrupt_and_run`（`crates/pod/src/interrupt_and_run.rs`）の中身は:

1. worker history に残る orphan な `Item::ToolCall`（前 turn が中断されて対応する `ToolResult` が未発行）に synthetic `ToolResult` を被せる（Anthropic 等の wire-validity 要件）
2. 「直前の作業は中断された」旨の system note を `worker` に push
3. 最後に `self.run(input)` に flow

つまり**入口の前処理が違うだけで、出口は通常の `pod.run` に合流する**。

しかもこの前処理が必要かどうかは、worker が一次情報として持っている `last_run_interrupted` フラグで判定できる（`pod.rs:1808` で既に使用済み）。`PodStatus::Paused` は controller がそれを観測したミラーに過ぎない。

結果として、Controller 層が Pod の内部状態を覗いて分岐するという責務漏れになっている。`PendingRun` enum は `Method::Run` 経由のものが 2 variant に分裂し、本来「parent originated か否か」など Run の意味論を扱うはずの enum に Pod 内部都合の分岐が混ざる。

## 要件

- `PendingRun::InterruptAndRun(input)` バリアントを削除し、`PendingRun::Run(input)` 一本に統合する。
- `controller_loop` の `Method::Run` ハンドリングは status を見ずに `PendingRun::Run(input)` を stage するだけにする。
- interrupt 前処理（orphan tool_result の closure + system note 挿入）は `Pod::run` の入口で `self.worker().last_run_interrupted()` を見て自発的に行う。
- `Pod::interrupt_and_run` メソッドは削除する。`interrupt_and_run.rs` のロジック（`orphan_tool_result_closures` 等）は `Pod::run` の前段として `pod.rs` 側、または同モジュールから `Pod::run` が直接呼ぶ形に再配置する。
- `Method::Resume`（`PendingRun::Resume`）の経路には影響させない。Resume は前 turn の context を生かして続行する別経路で、interrupt 前処理を入れてはいけない。
- 動作上の変化はゼロ（リファクタ）。Paused 中に `Method::Run { input }` が来たときの挙動は今と完全に同じであること。

## 完了条件

- `PendingRun` は `Run(Vec<Segment>)` / `RunForNotification` / `Resume` の 3 variant のみ。
- `controller_loop` の `Method::Run` ブランチに `status_before == PodStatus::Paused` 分岐が残っていない。
- `Pod::run` を Paused 直後（`last_run_interrupted == true`）の状態で呼ぶと、orphan `ToolCall` の closure と interrupt system note が history に積まれてから新規 input が処理される。これを既存の `interrupt_and_run` のテスト相当でカバーする。
- `Pod::run` を非 Paused 状態で呼ぶと従来通り interrupt 前処理は走らない。
- 既存の Paused 関連 E2E / 結合テストが全て通る。

## 範囲外

- `[[pod-parent-turn-callback]]` の `is_parent_originated` 判定ロジック自体の変更（本チケット完了後は `Run` / `Resume` の 2 variant が true になる形に自然に整理されるが、本チケットの目的ではない）。
- `worker.last_run_interrupted` のセマンティクス変更。
- TUI 側で Paused 中の送信を `Run` / `Resume` どちらにマップするかの UX 議論。

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./pod-interrupt-prep-internalize.review.md](./pod-interrupt-prep-internalize.review.md)
- 日付: 2026-05-15
