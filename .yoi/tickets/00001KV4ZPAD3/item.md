---
title: 'Panel Ticket / Intake Pod row の視覚階層を改善する'
state: 'closed'
created_at: '2026-06-15T06:32:21Z'
updated_at: '2026-06-15T14:59:39Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel-ux', 'tui-layout', 'accessibility', 'row-selection']
queued_by: 'workspace-panel'
queued_at: '2026-06-15T06:37:02Z'
---

## Background

Workspace Panel では、Ticket row が2行表示になり、Ticket に関連する Intake Pod row も Ticket の隣接 row として表示されるようになっている。

しかし現状では、以下の視覚的な区別が弱く、一覧として読んだときに情報の優先順位が掴みにくい。

- Ticket 本体の primary line と detail/gate line の強弱。
- Ticket row と associated Intake Pod row の親子関係。
- canonical state/title、gate/action/reason、Intake Pod status/open 導線の重要度差。
- 選択中 row/group と非選択 row/group の境界。

今回の作業は Panel 表示の visual hierarchy / readability 改善に限定する。Ticket lifecycle、local role/session registry、claim semantics、automatic launch/spawn policy は変更しない。

## Request snapshot

> 前やった Panel の2行レイアウトと、IntakePod の関連表示の件、表示の強弱がついてなくて見辛いから、改善したい。

Panel handoff:

- workspace: `yoi`
- workspace_orchestrator_pod: `yoi-orchestrator`

## Requirements

- Workspace Panel の Ticket 2行 row に視覚的な強弱を付ける。
  - 1行目の canonical state + title は primary 情報として読みやすくする。
  - 2行目の Ticket id / Gate / Action / reason は secondary 情報として、必要な時に読めるが主情報を邪魔しない表示にする。
- Ticket-associated Intake Pod row が、その Ticket の子/関連 row であると視覚的に分かるようにする。
  - indentation、prefix、dim style、role/status chip、group marker など、既存 Panel UI と整合する方法でよい。
- 選択状態では、Ticket 2行 + associated Intake row の関係が崩れない。
  - どの logical row / child row が選択されているかが分かる。
  - Ticket 本体と Intake Pod row の操作対象が混同されない。
- live / restorable / stale など Intake Pod status の表示は残しつつ、Ticket title/gate より過剰に目立たないようにする。
- narrow terminal / short panel area でも破綻しない。
  - truncate / ellipsis / bounded rendering を維持する。
- 色だけに依存しない表示にする。
  - terminal theme や monochrome 環境でも、indentation / marker / text label で最低限の関係が分かる。

## Acceptance criteria

- Ticket 2行 row で、primary line と secondary line の視覚的な強弱が確認できる。
- Ticket-associated Intake Pod row が、隣接する Ticket の関連/子 row として認識できる。
- Intake Pod row が Ticket 本体や別 Ticket に見えない。
- selected Ticket row / selected Intake Pod row の見え方が明確で、操作対象が混同されない。
- `ready`, `planning`, `queued/inprogress`, `done/closed`, `ready+waiting` の Ticket row で可読性が維持される。
- Intake Pod の `live` / `restorable` / `stale` status が確認できる。
- pre-Ticket Intake Pod を誤って特定 Ticket の child row のように表示しない。
- mouse click / keyboard selection の既存 semantics を壊さない。
- Focused tests で row rendering contract または ViewModel/row ordering/selection contract が確認される。

## Binding decisions / invariants

- Ticket lifecycle state / relation gate semantics は変更しない。
- persisted `waiting` state や新しい Ticket schema は追加しない。
- local Pod assignment / Pod name / socket / claim state / runtime status を git-tracked Ticket metadata/frontmatter/thread に保存しない。
- automatic polling / automatic Intake spawn は追加しない。
- Ticket と Intake Pod の関係を 1:1 と仮定しない。
- selected arbitrary Pod direct-send UX を復活させない。
- 表示改善のために lifecycle action の authority boundary を緩めない。

## Implementation latitude

- exact な UI 表現は実装側判断でよい。
  - dim / bold / color / prefix / indentation / separator / role chip / status chip など。
- 既存 `PanelRowKind::Ticket` / `PanelRowKind::TicketIntakePod` の rendering を調整してもよい。
- 必要なら shared row style helper を整理してよい。
- 色を使う場合でも、marker / label / indentation など非色要素で関係性が分かるようにする。
- Very small terminal で情報を減らす場合、primary state/title と action safety を優先する。

## Readiness

- readiness: implementation_ready
- risk_flags: [panel-ux, tui-layout, accessibility, row-selection]
- blocking open questions: none

## Escalation conditions

- Panel の selection model / keyboard semantics を大きく変える必要が出た場合。
- Ticket row と Intake Pod row の action identity が曖昧になり、誤操作リスクが出る場合。
- 色/装飾だけでは accessibility / terminal theme 上の問題を避けられない場合。
- Row rendering のために local role/session registry や Ticket schema の変更が必要になりそうな場合。
- bounded row rendering を維持できず、大量 Ticket/Pod で一覧が読みにくくなる場合。

## Validation

- `cargo test -p tui workspace_panel --lib`
- 関連箇所を触る場合:
  - `cargo test -p tui multi_pod --lib`
  - `cargo test -p tui row_hit_testing --lib`
  - `cargo test -p tui mouse_click --lib`
- `cargo fmt --check`
- `git diff --check`
- 可能なら実際の `yoi panel` / PTY で目視確認、または既存 Panel E2E の更新。

## Related work

- `00001KV12W2RT` — Panel Ticket rows を2行表示にして gate 情報を分離する
- `00001KV09WYC6` — Workspace panel: show Ticket-associated Intake Pods adjacent to Ticket rows
- `00001KV3A5CNH` — Panel: invalid Ticket があっても Ticket 機能全体を無効化しない

Code areas:

- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/role_session_registry.rs`
