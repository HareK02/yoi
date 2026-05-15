# Review: Paused→Run の interrupt 前処理を Pod 内部に閉じる

対象コミット: `62af74c update: Paused→Run の interrupt 前処理を Pod::run に内包`

## 前提・要件の確認

- **`PendingRun::InterruptAndRun(input)` バリアントを削除し `PendingRun::Run(input)` 一本に統合する**: 達成。`crates/pod/src/controller.rs:97` の `PendingRun` 定義から `InterruptAndRun` が削除され、`is_parent_originated` (`crates/pod/src/controller.rs:109`) と `pending_run_parent_origin_table` テスト (`crates/pod/src/controller.rs:940-944`) もそれに合わせて整理されている。
- **`controller_loop` の `Method::Run` ハンドリングは status を見ずに `PendingRun::Run(input)` を stage するだけにする**: 達成。`crates/pod/src/controller.rs:584-611` で `status_before` バインドが削除され、`shared_state.get_status() == PodStatus::Running` の防御チェックは残しつつ、Paused 判定の分岐は完全に消えている。
- **interrupt 前処理は `Pod::run` 入口で `self.worker().last_run_interrupted()` を見て自発的に行う**: 達成。`crates/pod/src/pod.rs:1155-1157` で `worker.last_run_interrupted()` をゲートとして `apply_interrupt_prep` を呼んでいる。前処理本体は `crates/pod/src/pod.rs:1422-1442`。
- **`Pod::interrupt_and_run` は削除し、`orphan_tool_result_closures` 等を `Pod::run` の前段として再配置**: 達成。`crates/pod/src/interrupt_and_run.rs` が `crates/pod/src/interrupt_prep.rs` にリネームされ、`pub(crate)` の純関数 `orphan_tool_result_closures` のみを公開する形に縮退。`Pod::run` 側 (`apply_interrupt_prep`) が直接これを呼ぶ。`lib.rs:14` のモジュール宣言も `interrupt_prep` に更新。
- **`Method::Resume`（`PendingRun::Resume`）の経路には影響させない**: 達成。`Pod::resume` (`crates/pod/src/pod.rs:1542-1554`) は `apply_interrupt_prep` を呼ばないため、Resume 経路から interrupt 前処理は発火しない。`do_compact_and_resume` の再帰 (`crates/pod/src/pod.rs:1687`) も `resume()` 側に閉じている。
- **`run_for_notification` も無関係**: 確認済み。`crates/pod/src/pod.rs:1528-1539` は `prepare_for_run` → `worker.resume()` の経路で `Pod::run` を経由しないため、Notify 経路にも前処理は混入しない。
- **動作上の変化はゼロ（リファクタ）**: 達成。
  - 前処理の中身（`orphan_tool_result_closures` のロジック、`extend_history` での closures push、`push_item` での system_note push）はバイト単位で一致。
  - 順序も維持: 旧 `interrupt_and_run` では `validate → extend(closures) → push(note) → self.run(input)` で、`run` 内部で `prepare_for_run → commit_entry(UserInput)` が走る。新 `Pod::run` では `validate → apply_interrupt_prep → prepare_for_run → commit_entry(UserInput)` と同じ並び。
  - 旧コードに残っていた `interrupt_and_run` 側の重複 `validate_workflow_invocations` は自然に解消（旧コメントが「duplicate call is cheap (read-only) and collapses naturally once interrupt_and_run folds into Pod::run」と予告していた通り）。
- **完了条件のテスト**: `paused_then_run_closes_orphan_tool_use_for_next_request` (`crates/pod/tests/controller_test.rs:1312`) が新経路でも従来通り orphan closure + system note + 新 user message を 2 nd LLM request に詰めることを検証しており、コメントも `last_run_interrupted` ベースに更新済み。`interrupt_prep` の純粋ロジック 4 テストも `cargo test -p pod --lib interrupt_prep` で全て pass。

## アーキテクチャ・スコープ

- 「Controller が Pod 内部状態 (`PodStatus::Paused`) を覗いて分岐していた責務漏れ」が解消されている。`PendingRun` enum が「parent originated か否か」「Run / Resume / RunForNotification」という Run の意味論だけを表すよう純化された。
- `interrupt_prep.rs` は `orphan_tool_result_closures` のみを `pub(crate)` で公開する単機能モジュール。`Pod::apply_interrupt_prep` 自体は `pod.rs` 側に置かれ、prompt catalog 引きと worker 操作という Pod 固有の責務を `Pod` impl に集約している。モジュール分割の粒度として妥当（インライン化すると `pod.rs` が膨らみ過ぎ、逆に `interrupt_prep` 側に押し込むと catalog アクセスを跨ぐ）。
- `Pod::run` の冒頭は `validate → interrupt prep → prepare_for_run → commit UserInput → flatten → lock+run` の素直なシーケンスになり、コメントも更新済み (`crates/pod/src/pod.rs:1146-1157`, `1413-1419`)。
- llm-worker 層には触っていない。`last_run_interrupted` は既存 API のまま読み取りに使われており、層境界を歪めていない。
- 範囲外 (`is_parent_originated` の表現変更、`last_run_interrupted` セマンティクス変更、TUI 側議論) は変更していない。

## LLM コンテキスト加工原則

- orphan tool_result closure と interrupt system note は `extend_history` / `push_item` で `worker.history` に **append** されており、context-only な差し込みではない。CLAUDE.md の「許される変換 vs 禁止される非再現的差し込み」の禁止側には該当しない。
- 注意: これらは旧 `interrupt_and_run` と同じく `extend_history` (callback なし) と `push_item` を使うため、`Worker::on_history_append` 経由の `LogEntry::ToolResult` / `LogEntry::SystemItem` への即時 commit は走らない。`persist_turn` のフォールバック側でも `history_before` の手前に置かれているため拾われない。結果として「worker history (in-memory) には載るが session-log には独立エントリとして残らない」状態であり、これは本コミット以前から同じ振る舞いで、本ティケットの「動作上の変化はゼロ」要件を満たしている。session-log 永続化のセマンティクスは別のチケットで扱うべき独立した論点。

## 指摘事項

### Blocking
- なし。

### Non-blocking / Follow-up
- **interrupt prep 由来の history append が session-log に独立エントリとして残らない問題**: 旧実装からの引き継ぎで、本チケットでは正しく不変。ただし「Notify / PodEvent / `<system-reminder>` 系は history に append して commit する」という CLAUDE.md の原則と整合させるかは将来のチケット (`notify-history-persist` 系の流儀に合わせる) として検討する余地がある。本ティケットの責務外。
- **`controller_test.rs::double_run_returns_error` の flakiness**: 5 回中 1 回程度 fail する timing-sensitive テスト。本コミットの変更とは独立で、`Method::Run` ブランチの構造変更は `status_before` バインドの除去のみで意味的影響なし（`PodStatus::Running` での AlreadyRunning 防御も維持）。旧コミットでも同等の race window が存在していた。観測のため記録のみ。

### Nits
- `crates/pod/src/interrupt_prep.rs:14-16` の `use` 順が `cargo fmt` 既定と不一致（`crate::` を `llm_worker::` より先に置くのが rustfmt 既定）。`cargo fmt -p pod --check` が当該箇所で diff を出す。本コミットで新規導入された唯一のフォーマット差分。`fmt -p pod` をかければ閉じる。
  - なお `pod.rs` / `notify_buffer.rs` の `})?;` 改行も `fmt --check` で diff になるが、これらは `62af74c^` (`d076258`) 時点で既に存在する別件で、本コミットの責任範囲ではない。

## 判断

**Approve with follow-up** — 要件は全て満たされ、コードベースの責務分離もきれいに整った。動作変更ゼロのリファクタとして十分。`interrupt_prep.rs` の `use` 順だけ気が向いたら `cargo fmt` を当てておく程度の nit が残るのみで、ブロッキングは無い。

