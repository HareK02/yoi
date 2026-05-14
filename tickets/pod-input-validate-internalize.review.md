# Review: Run 入力の事前 validate を Pod 内部に閉じる

## 前提・要件の確認

- 要件1「`Pod::run` の冒頭、`commit_entry(LogEntry::UserInput { .. })` よりも前に `validate_workflow_invocations` を呼び、エラー時は session log に何も commit せず `PodError` で early return」: 満たされている。`crates/pod/src/pod.rs:1138-1145` で `self.validate_workflow_invocations(&input)?;` が `prepare_for_run` よりも前、すなわち `ensure_session_head` / pre-run compact / `commit_entry(UserInput)` のいずれよりも手前にある。`?` で `WorkflowResolveError` が `PodError::WorkflowResolve` に lift され early return される。
- 要件2「`controller_loop` の `Method::Run` ハンドリングから事前 validate 呼び出しを削除」: 満たされている。`crates/pod/src/controller.rs:600-630` の `Method::Run` ブランチに validate 呼び出しは存在しない。
- 要件3「`PodError` を `worker_error_code` で `ErrorCode::InvalidRequest` にマップ」: 満たされている。`crates/pod/src/controller.rs:944` に `PodError::WorkflowResolve(_) => ErrorCode::InvalidRequest` 分岐が追加され、ticket が示した「既存バリアントの再利用」方針通り `WorkflowResolveError` を新規ラップせずに既存 `PodError::WorkflowResolve(#[from] WorkflowResolveError)` (`pod.rs:3177-3178`) を流用している。
- 要件4「`Pod::validate_workflow_invocations` の可視性は `pub` のまま」: 満たされている (`pod.rs:1407`)。docstring は controller 経路ではなく `Pod::run` / `Pod::interrupt_and_run` 入口で呼ぶ旨に更新済 (`pod.rs:1402-1406`)。

## 完了条件の確認

- 「`controller.rs` の `Method::Run` ブランチに `pod.validate_workflow_invocations` の呼び出しが残っていない」: OK (`grep` で残存呼び出しなし、Pod::run/interrupt_and_run のみ参照)。
- 「不正な workflow slug を含む `Method::Run` を投げると、UserInput が session log に commit されないまま `Event::Error { code: InvalidRequest, .. }` が flow する」: OK。`Pod::run` 入口の `validate_workflow_invocations?` が `prepare_for_run` / `commit_entry(UserInput)` よりも手前で抜けるため session log には何も書かれない。`drive_turn` の `Err(e)` ブランチが `worker_error_code` 経由で `InvalidRequest` を載せた `Event::Error` を broadcast する。
- 「既存テスト通過」: `cargo test -p pod --lib` 173/173 通過 (再現確認済)。`workflow::tests::{resolves_requires_before_workflow_body, user_invocable_false_errors, missing_required_knowledge_errors}` は `resolve_workflow_invocation` の関数単体テストで本 refactor の影響を受けない。

## アーキテクチャ・スコープ

- 責務漏れの是正方向は妥当: controller が Pod 内部の `workflow_registry` を覗いて条件分岐していたのを Pod の入口に集約。read-only 関数を本来の所有者側で呼ぶだけの最小変更。
- 範囲外項目 (resolve ロジック / 判定基準 / Resume / RunForNotification) には踏み込んでいない。`run_for_notification` / `resume` は user 入力 segments を受け取らないので validate 経路に乗らない点もコードと整合。
- `interrupt_and_run` 側にも validate を入れた判断は妥当。チケットには明示されていないが、ticket 背景にある同じ責務漏れパターン (`pod-interrupt-prep-internalize`) と整合させるため必要。validate 抜きで `interrupt_and_run` を呼ぶと orphan tool_result closure / interrupt system note が worker history に書き込まれた後で `Pod::run` 入口の validate に弾かれ、half-applied 状態が残る。`interrupt_and_run.rs:31-37` のコメントが `pod-interrupt-prep-internalize` 統合時に自然に重複解消する旨を明示しており、将来の畳み込みに対する負債もない。
- `worker_error_code` の新規 `WorkflowResolve` 分岐は副作用として post-commit な resolve エラー (`KnowledgeNotFound` / `KnowledgeRead` / `KnowledgeFrontmatter`) も `InvalidRequest` にマップする。これらは以前 `_ => Internal` に落ちていたため挙動変化だが、意味論的にはより適切方向であり、validate / resolve でバリアントを分けるのは本チケットの範囲を超える。許容範囲。

## 指摘事項

### Non-blocking / Follow-up

- `Event::UserMessage` 早期 broadcast 起因の UX 差分: 旧実装では validate 失敗時、`Event::UserMessage` も `Event::Status { Running }` も飛ばなかった。新実装では bad slug の場合でも UserMessage → Status(Running) → Error → Status(Idle) が並ぶ。TUI (`crates/tui/src/app.rs:478-485`) は `UserMessage` 受信で `turn_index += 1` し TurnHeader + UserMessage block を push するため、エラーが返るだけのリクエストでも turn 番号が前進し、ターンヘッダと user line がエラーブロックの上に残る。チケットの完了条件 (session log への UserInput commit 抑止) には抵触しないが、`bad slug → ghost turn header が残る` 表示挙動は旧実装と異なる。フォローアップで controller 側の `Method::Run` ハンドリングを「validate を controller でも呼ばずに Pod 側に丸投げするのは維持しつつ、UserMessage broadcast は drive_turn の中で投げる」「Pod::run 内で resolve 成功直後に Event::UserMessage 相当を内側から飛ばす」等の動線整理を検討する余地がある。本チケットの責務漏れ修正としては完了範囲外と判断。
- `worker_error_code` の `WorkflowResolve` 分岐コメント: validate-stage 失敗 (InvalidSlug / NotFound / NotUserInvocable) と resolve-stage 失敗 (KnowledgeNotFound / KnowledgeRead / KnowledgeFrontmatter) が同じ `InvalidRequest` に丸まる旨を一行コメントで明示しておくと将来の差分判断が楽になる。Nit に近い。

### Nits

- なし。

## 判断

Approve with follow-up — チケットの前提・要件・完了条件は全て満たされ、責務漏れの修正方向も妥当。`Event::UserMessage` 早期 broadcast による turn 表示差分は許容範囲のトレードオフ (実装者ノート通り) で、本チケットの完了範囲外として別途整理する余地のみ残る。
