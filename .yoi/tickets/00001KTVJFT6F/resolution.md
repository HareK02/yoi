Workspace panel の focus model を composer target と row selection に整理した。

実装概要:
- `yoi panel` の user-visible focus 表示から `item action focus` / `Right action focus` / `global composer` / `PanelFocus` / `ItemAction` 系の不要な focus model を除去した。
- composer target は送信先、row selection は空 composer 時の navigation / Enter 対象として扱う表示・挙動へ整理した。
- 非空 composer では composer draft / target を優先し、`Enter` は composer send / Intake 起動に向く。
- 空 composer では selected row が `Enter` 対象になり、既存 Ticket action dispatch / Pod open 経路を使う。
- `Tab` は composer target の切替のみで selected row と draft を保持する。
- `Esc` は row selection を解除し、composer draft と target は保持する。
- `Left` / `Right` は Panel focus 切替ではなく composer cursor 操作として扱う。
- Ticket action dispatch、Pod open、Intake launch、Companion send の authority / safety semantics は維持した。

Review / integration:
- Implementation commit: `c5ef6f79 tui: clarify panel composer target and row selection`
- Reviewer: `yoi-reviewer-panel-focus-model` が approve。
- Orchestrator merge commit: `d6166c72 merge: panel focus composer row selection`
- Ticket completion commit: `e330685e ticket: mark panel focus done`

Validation:
- `cargo test -p tui selected_ticket_row_with_non_empty_composer_shows_composer_enter_behavior`: pass
- `cargo test -p tui multi_esc_clears_row_selection_without_quitting_and_preserves_draft`: pass
- `cargo test -p tui multi_composer_target_switch_preserves_typed_text`: pass
- `cargo test -p tui multi_blank_ticket_intake_enter_uses_selected_row_and_preserves_input`: pass
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known unrelated validation failure:
- `cargo test -p tui multi_ --lib` still includes pre-existing failure `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`; this was verified on the Orchestrator branch before merge and reviewed as unrelated to this Ticket.

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/panel-focus-composer-row-selection` removed。
- branch `ticket/panel-focus-composer-row-selection` deleted。

Non-blocking risks:
- Reviewer found none for this Ticket.