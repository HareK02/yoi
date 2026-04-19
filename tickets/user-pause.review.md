# user-pause レビュー

## 結論

主要ロジック（protocol / controller / Pod / TUI）は仕様通りで、ビルドと既存テストはすべて green。**ただしチケット完了条件で明示されている統合テスト 3 ケースが未実装のため、現状では完了条件未達**。

## 良い点

- `protocol` の `Method::Pause` variant 追加と serde round-trip テスト
- `controller.rs::run_with_cancel_support` の `pause_requested` フラグ設計と `PodEvent::Errored` の upward 抑止が仕様通り
- `Method::Pause` の Idle / Paused 区別（NotRunning エラー / no-op）
- `interrupt_and_run.rs` の責務分離: orphan 検出を純粋関数 `orphan_tool_result_closures` に切り出し、ユニットテスト 4 ケースで境界条件を網羅
- TUI の Ctrl-C 2 連打 UX と Ctrl-D との対称性。`paused` フラグを `Event::TurnStart` でクリアする経路は Resume / Run 両方でカバーされる
- `docs/tui-keybindings.md` が人間向け解説として丁寧。Cancel と Pause の使い分け、`Ctrl-R` / `Esc` 廃止経緯まで言及

## 指摘事項

### 1. 統合テスト不足（completion blocker）

チケット完了条件のうち、以下 3 項目に対応する統合テストが `crates/pod/tests/controller_test.rs` に追加されていない：

- 実行中 Pod に `Method::Pause` を送ると `PodStatus::Paused` に落ち、`Method::Resume` で続きから再開できる
- Paused → Run 後、LLM への送信が wire 上正しい（orphan `tool_use` が解消されている）
- Pause → Resume の history consistency

`interrupt_and_run.rs` のユニットテストは関数単位の検証であり、controller を通したエンドツーエンドの確認とは別物。既存の `resume_without_pause_returns_error` と同形式で 3 ケース追加すること。

### 2. ライフサイクル運用

`tickets/user-pause.md`, `crates/pod/src/interrupt_and_run.rs` 等が untracked / unstaged。CLAUDE.md のチケット運用（作成 commit → 詳細化 commit → レビュー commit）に沿うなら、実装と分けて commit すべき。

## 判定

統合テスト 3 ケースを追加した時点で完了条件達成。それまでは未完了。
