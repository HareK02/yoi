Implemented, reviewed, merged, and validated.

Summary:
- Clarified Panel focus and composer key handling.
- Moved Panel target switching from `Ctrl+T` to `Tab`; `Ctrl+T` no longer switches Panel targets or appears in Panel help/actionbar as target switch.
- Added shared `composer_keys` handling used by normal TUI and workspace panel composer editing.
- Preserved bare-letter typing behavior; letters such as `j/k/o/r` enter composer text rather than acting as shortcuts.
- Made focus/Enter behavior clearer for global composer, selected row, and item action states.
- Fixed selected Ticket row + non-empty global composer ambiguity: actionbar/status now describes composer-target Enter behavior rather than row action.
- Added regression tests and updated Panel canonical-ID display tests.

Implementation:
- Coder commits: `20f06b3 tui: clarify panel focus and composer keys`, `573b02f tui: clarify panel composer enter hints`
- Reviewer approved after fix loop.
- Merge commit: `57ed405 merge: improve panel composer keys`

Validation after merge:
- `cargo test -p tui selected_ticket_row_with_non_empty_composer_shows_composer_enter_behavior`
- `cargo test -p tui multi_ctrl_t_does_not_switch_composer_target`
- `cargo test -p tui multi_bare_panel_letters_append_to_composer_and_arrows_select_when_blank`
- `cargo test -p tui multi_esc_clears_panel_focus_without_quitting`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract`
- `cargo test -p tui panel_ticket_rows_use_aligned_columns_before_title`
- `cargo test -p tui panel_ticket_title_truncates_after_stable_columns`
- `cargo test -p tui` (291 passed)
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
