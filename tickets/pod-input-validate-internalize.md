# Run 入力の事前 validate を Pod 内部に閉じる

## 背景

`controller_loop` は `Method::Run { input }` を受けた直後、`drive_turn` に流す前に `pod.validate_workflow_invocations(&input)` を呼んで invalid なら `ErrorCode::InvalidRequest` を返している (`crates/pod/src/controller.rs:613`):

```rust
if let Err(e) = pod.validate_workflow_invocations(&input) {
    let _ = event_tx.send(Event::Error {
        code: ErrorCode::InvalidRequest,
        message: e.to_string(),
    });
    continue;
}
```

これは Pod 内部の `workflow_registry` を覗いてバリデーション判定する責務漏れになっている。`pod.validate_workflow_invocations` は read-only 関数で、`Pod::run` の冒頭でも当然呼べる。

なぜ controller が肩代わりしているかというと、`Pod::run` (`crates/pod/src/pod.rs:1138`) は冒頭で:

```rust
self.commit_entry(LogEntry::UserInput { ts: ..., segments: input.clone() })?;
self.user_segments.push(input.clone());
// ...
attachments.extend(self.resolve_workflow_invocations(&input)?);
```

の順で動く。`resolve_workflow_invocations` が失敗を返すころには既に UserInput が session log に commit されてしまっており、「invalid な input が history に残る」状態になる。それを避けるために controller 側で先打ち validate している。

[[pod-interrupt-prep-internalize]] と同じパターン: Pod 内部の整合性を保つ前処理が Controller 層に染み出している。

## 要件

- `Pod::run` の冒頭、`commit_entry(LogEntry::UserInput { .. })` よりも前に `validate_workflow_invocations` を呼び、エラー時は session log に何も commit せず `PodError` で early return する。
- `controller_loop` の `Method::Run` ハンドリングから事前 validate 呼び出しを削除する。
- `Pod::run` から返る `PodError`（新規に追加する workflow validation 失敗のバリアントを含む）を `worker_error_code` 等の controller 側エラーコードマッピングで `ErrorCode::InvalidRequest` にマップする。
  - `PodError` に既存の InvalidRequest 相当バリアントがあればそれを使う。無ければ `WorkflowResolveError` をラップする最小限のバリアントを追加する。
- `Pod::validate_workflow_invocations` メソッドの可視性は `pub` のままでよい（IPC `ListCompletions` 経路や他テストから参照される可能性があるので外向き API を狭めない）。

## 完了条件

- `controller.rs` の `Method::Run` ブランチに `pod.validate_workflow_invocations` の呼び出しが残っていない。
- 不正な workflow slug を含む `Method::Run` を投げると、UserInput が session log に commit されないまま `Event::Error { code: InvalidRequest, .. }` が flow する（既存の挙動と同等）。
- 既存の workflow invocation 関連テスト（成功 / NotFound / NotUserInvocable / InvalidSlug）が通る。

## 範囲外

- `resolve_workflow_invocations` 側のロジック変更。
- `validate_workflow_invocations` の判定基準変更（user_invocable / slug parse 等）。
- `Method::Run` 以外の経路（Resume / RunForNotification）からの入力検証。Resume は入力を取らず、RunForNotification の入力は notify buffer drain で system message として入るため workflow invocation の経路に乗らない。

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./pod-input-validate-internalize.review.md](./pod-input-validate-internalize.review.md)
- 日付: 2026-05-15
