---
title: 'TUI Dashboard の冗長な key hints と selected-row 状態表示を削る'
state: 'ready'
created_at: '2026-06-23T05:40:56Z'
updated_at: '2026-06-23T05:40:56Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-ux', 'terminal-layout']
---

## User claims / request snapshot

- ユーザーは TUI Dashboard の key hints を消したい。
- 対象として、Composer 下部の hint、最上行の hint、最上行側に出ている selected-row / 選択中表示が挙げられた。
- 選択中の情報は Composer 側で賄えているため、別表示はいらない、という意図。

## Confirmed facts / sources

- `crates/tui/src/dashboard/render.rs` の `draw_title` が最上行の guidance/hint を描画している。
- `crates/tui/src/dashboard/render.rs` の `target_status_line` が composer target と selected Ticket / selected Pod / selected Intake Pod / no row selected などの状態行を描画している。
- `crates/tui/src/dashboard/render.rs` の `actionbar_left_text` / `actionbar_right_text` が Composer 下部の actionbar/key hints を描画している。
- `crates/tui/src/dashboard/render.rs` の `panel_row_title_line` / `push_ticket_primary_marker_span` などは list 内の選択マーカーを描画しており、今回の「最上行の選択中表示」とは分けて扱うのが安全そう。
- closed Ticket `00001KTFQ109V` / `Remove workspace panel direct Pod send` は direct Pod send の UI/key hints を除去済みで、今回の依頼と矛盾しない。

## Unverified hypotheses

- 「最上行の選択中の表示」は、list 内の `▶` marker ではなく、`target_status_line` に出ている selected Ticket / selected Pod などの textual status を指している、という前提。
- Composer 下部の「奴」は `draw_actionbar` で描画される actionbar 全体、または少なくとも右側の key hint string を指している、という前提。

## Undecided points / open questions

- blocking open question はない。
- 実装時の確認前提: list 内の選択マーカー `▶` / selected row の色・bold は残す。消すのは説明文・key hint・選択中状態のテキスト表示であり、キーボード操作自体は変えない。

## Background

Dashboard は composer target と row selection の関係を複数箇所で説明しているが、現在は Composer 側で十分に文脈を伝えられるため、画面上の hint / status 表示が冗長になっている。

## Requirements

- Dashboard の最上行から key hint / 操作説明の guidance を削る。
- Composer 下部の actionbar から常時表示される key hints を削る、または最小化する。
- selected Ticket / selected Pod / selected Intake Pod / no row selected などの専用 status line 表示を削る。
- Composer target / draft submit の必要な情報は Composer 近傍または既存 composer 表示で賄う。
- row selection / keyboard operation の実際の挙動は維持する。
- Ticket Queue / Intake launch / Pod open-attach / Companion target の既存動作は変えない。

## Acceptance criteria

- `yoi panel` の最上行に `Row selection`、`blank Enter`、`Tab target` 等の key hint 文言が出ない。
- Composer 下部の actionbar に常時 key hint 群が出ない。
- selected row の textual status line が出ない、または selected-row 表示として認識される冗長情報がなくなる。
- list 内では現在の選択行を視認できる。
- 既存の Dashboard 操作は維持される:
  - blank Enter の row action
  - text Enter の composer target action
  - Tab target switching
  - Esc clear selection
  - Pod open/attach
  - Ticket Intake / Queue 関連動作
- 関連 render/unit tests が新しい表示仕様に更新される。

## Binding decisions / invariants

- この Ticket では Dashboard の表示整理だけを扱う。
- Console / single-Pod TUI の key hints は対象外。
- list 内の選択行マーカーや keyboard navigation visibility は削らない。
- direct selected-Pod send を再導入しない。
- Companion lifecycle / Orchestrator lifecycle / Ticket workflow semantics は変更しない。

## Implementation latitude

- `target_status_line` の layout row を完全に削るか、空/最小表示にするかは実装側で判断してよい。
- actionbar は完全削除・左側 notice のみ残す・一時 notice のみ残す等、既存 layout とテストに自然な形を選んでよい。
- 既存 tests は snapshot 的に文字列を確認している箇所を中心に更新してよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-ux, terminal-layout]

## Escalation conditions

- row selection marker まで削る必要が出た場合。
- Composer だけでは current target / action が分からなくなり、代替表示が必要だと判断した場合。
- layout row を削ることで terminal resize / hitbox / mouse selection に副作用が出る場合。

## Validation

- `cargo test -p tui dashboard --lib` または Dashboard 関連の focused tests。
- 必要に応じて `cargo test -p tui workspace_panel --lib`。
- `cargo fmt --check`
- `git diff --check`
- 可能なら `yoi panel` の実表示確認。

## Related work

- `00001KTFQ109V` — Remove workspace panel direct Pod send
- `crates/tui/src/dashboard/render.rs`
- `crates/tui/src/dashboard/mod.rs`
- `crates/tui/src/dashboard/tests.rs`
