---
title: 'Dashboard reload と初期表示で row を自動選択しない'
state: 'inprogress'
created_at: '2026-06-23T06:44:20Z'
updated_at: '2026-06-23T07:23:02Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-ux', 'panel-selection', 'reload-state']
queued_by: 'workspace-panel'
queued_at: '2026-06-23T06:47:17Z'
---

## User claims / request snapshot

- Dashboard で `Esc` を押して row selection を消しても、reload によって頻繁に選択が戻る。
- そのため、実使用上は「何も選択していない状態」で composer を global / 新規 Intake として使うのが難しい。
- reload で勝手に row selection しないようにしたい。
- 追加合意: 初回 `yoi panel` 起動時から未選択にする。

## Confirmed facts / sources

- `crates/tui/src/dashboard/mod.rs`
  - Dashboard は `selected_row: Option<PanelRowKey>` を持ち、`None` の未選択状態が実装上存在する。
  - `Esc` 経路で `clear_panel_selection()` が呼ばれ、selection を消せる。
  - reload / snapshot 更新後の selection visibility 補正により、visible row があると selection が戻る可能性がある。
- `crates/tui/src/dashboard/tests.rs`
  - `Esc` が row selection を clear する既存テストがある。
- 関連 Ticket:
  - `00001KTNS1AA8` closed: Panel composer / no-selection / `Esc` semantics の過去文脈。
  - `00001KTFMMZP0` closed: background reload と selection/composer preservation の過去文脈。

## Unverified hypotheses

- 現行の reload 完了時の `ensure_selection_visible()` 系処理が、明示的な no-selection と「まだ selection がない初期/補正状態」を区別していない可能性が高い。
- 初期表示時の自動選択も同じ selection visibility 補正または初期化経路に由来している可能性がある。

## Undecided points / open questions

- なし。初回表示から未選択にする方針で合意済み。

## Background

Dashboard composer は row selection と composer target の組み合わせで挙動が変わる。`TicketIntake` target で Ticket row が選択されていると、入力が既存 Ticket の詳細化 / planning return として扱われる経路がある。一方、ユーザーが global / 新規 Intake として入力したい場合は row selection がない状態を維持できる必要がある。

現状は `Esc` で selection を消せても、background reload 等で選択が戻るため、ユーザーが意図した no-selection 状態を維持しづらい。

## Requirements

- 初回 `yoi panel` 表示時は、Ticket row / Pod row を自動選択しない。
- ユーザーが `Esc` で row selection を clear した後、reload / background reload / snapshot 更新が完了しても `selected_row = None` を維持する。
- reload によって既存の明示選択がまだ visible な場合は、その selection を維持してよい。
- reload によって選択中 row が消えた場合は安全に未選択へ落とす。
- Up/Down、mouse click、その他既存の明示的な row selection 操作では従来どおり selection を作れる。
- 未選択状態で `TicketIntake` target に非空 composer 入力して Enter した場合、既存 Ticket refinement ではなく global / 新規 Intake launch として扱われる。

## Acceptance criteria

- 初回 `yoi panel` 起動後、visible row があっても row selection はない。
- `Esc` で selection を clear した後、panel reload completion を挟んでも selection は復活しない。
- 未選択状態で `TicketIntake` target に composer 入力して Enter すると、新規/global Intake handoff になり、既存 Ticket の詳細化 handoff にならない。
- keyboard navigation または mouse click により、ユーザーは明示的に row を再選択できる。
- 既存の selected-row action、queue/close/open 操作、Ticket workflow state semantics は変えない。
- focused tests で少なくとも以下を確認する:
  - initial Dashboard state has no selected row when rows exist;
  - `Esc` clear -> reload completion -> still no selected row;
  - no-selection + `TicketIntake` composer submit routes to global/new Intake rather than selected Ticket refinement.

## Binding decisions / invariants

- no-selection は first-class UX state として扱う。
- reload は、ユーザーが明示的に作った no-selection state を勝手に解除しない。
- 初回表示でも自動選択しない。
- composer draft は reload / selection clear によって失わない。
- Ticket state transition / lifecycle authority は変更しない。
- row click は selection のみを変更し、click だけで action dispatch しない既存方針を維持する。

## Implementation latitude

- 実装は `selected_row: Option<PanelRowKey>` に加えて「明示 no-selection」フラグを持つ、または selection visibility 補正の呼び出し条件を整理するなど、局所的に選んでよい。
- 初期表示・reload 完了・row 消失・navigation 開始の各経路で、selection を作る責務を明示的 user action に寄せられればよい。
- UI 表示文言の小変更は許容するが、この Ticket の主目的は selection semantics の修正であり、広い Dashboard layout redesign は含めない。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-ux, panel-selection, reload-state]

## Escalation conditions

- 初回未選択にすると keyboard-only navigation や blank Enter action の既存 contract が大きく変わることが判明した場合。
- selection visibility 補正が reload 以外の重要な safety/correctness を担っており、単純に止めると stale selection や action 誤爆が起きる場合。
- focused tests では確認できず、実端末 `yoi panel` / PTY 経路での挙動確認が必要になった場合。

## Validation

- `cargo test -q -p tui workspace_panel`
- 必要に応じて Dashboard 関連の focused test 名指定。
- 変更範囲に応じて `cargo fmt --check`、`git diff --check`、`cargo check -q`。
- terminal delivery / timing に依存する挙動が疑わしい場合は、既存の Panel PTY/E2E 系テストまたは実 `yoi panel` で確認する。

## Related work

- `00001KTNS1AA8` — Panel composer / no-selection / `Esc` semantics の関連 closed Ticket。
- `00001KTFMMZP0` — Dashboard background reload / selection preservation の関連 closed Ticket。
- `crates/tui/src/dashboard/mod.rs`
- `crates/tui/src/dashboard/tests.rs`
- `crates/tui/src/dashboard/render.rs`
