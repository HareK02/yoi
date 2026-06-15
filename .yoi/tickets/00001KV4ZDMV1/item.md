---
title: 'Panel composer で Alt+Enter 改行を SessionView と揃える'
state: 'ready'
created_at: '2026-06-15T06:27:36Z'
updated_at: '2026-06-15T06:27:36Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-input', 'ux-consistency']
---

## Background

Panel の Composer でも、SessionView / 通常チャット composer と同じように `Alt+Enter` で改行できるようにする。

既存 Ticket `00001KTNS1AA8` で通常 TUI と workspace Panel の composer key handling を共有する方向が実装・完了済み。thread には shared editing が `Alt+Enter newline` を含む旨も記録されている。

一方、現行の Panel key handling では `KeyCode::Enter` の submit/open 分岐が `composer_edit_action(key)` より先に処理されているため、`Alt+Enter` が `InsertNewline` として到達せず、通常 Enter と同じ挙動になる可能性がある。Panel composer でも SessionView と同じ入力体験に揃える。

## Requirements

- Panel composer で `Alt+Enter` が draft 内の改行挿入として動作する。
- `Alt+Enter` は composer text の submit、Ticket Intake launch、Companion send、selected row open/dispatch を起こさない。
- 通常の bare `Enter` の既存 semantics は維持する。
  - composer が空なら selected row / open / dispatch。
  - composer に text があるなら current composer target への submit。
- 通常 SessionView 側の `Alt+Enter` 改行挙動を壊さない。
- 既存の shared `composer_keys` 方針を保ち、Panel 専用の ad-hoc key handling を増やしすぎない。

## Acceptance criteria

- `yoi panel` の composer に text がある状態で `Alt+Enter` を押すと、draft に newline が入る。
- composer が空、row selected、Ticket action selected などの Panel 状態でも、`Alt+Enter` は submit/open/dispatch ではなく改行編集として扱われる。
- bare `Enter` の Panel submit/open/dispatch behavior は regress しない。
- focused test で `Alt+Enter` が Panel composer newline を挿入し、submit action を返さないことを確認する。
- 既存 SessionView / normal TUI composer key tests が通る。

## Binding decisions / invariants

- `Alt+Enter` は composer editing key として扱い、Panel action key として扱わない。
- bare `Enter` と `Alt+Enter` の意味を明確に分ける。
- Panel composer は通常 TUI composer と同じ shared composer key handling 方針に従う。
- Panel row selection / Ticket action semantics、Companion / Ticket Intake target semantics は変更しない。
- bare letter shortcuts は復活させない。

## Implementation latitude

- `composer_edit_action(key)` を Enter submit/open 分岐より前に適用する、または `KeyCode::Enter` 分岐側で modifiers を明示的に見て `Alt+Enter` を除外するなど、実装方法は任せる。
- Existing tests に追加する形でも、新規 focused regression test を作る形でもよい。
- UI hint の更新は必要なら最小限に行う。主目的は key behavior の修正。

## Readiness

- readiness: implementation_ready
- priority: P2 相当
- risk_flags: [tui-input, ux-consistency]
- open_questions: none

## Escalation conditions

- terminal / crossterm が対象環境で `Alt+Enter` を識別できない場合は、実装可能範囲と制約を implementation report に明記する。
- `Alt+Enter` を先に composer edit として処理すると、既存 completion / target switching / row action の前提と衝突する場合は Orchestrator / reviewer に相談する。
- SessionView 側にも同様の問題がある場合は、Panel 固有 bug ではなく shared composer key handling の regression として扱う。

## Validation

- Focused test: Panel composer で `Alt+Enter` が newline を挿入し、submit/open/dispatch しない。
- Focused test: bare `Enter` の既存 Panel behavior が維持される。
- `cargo test -p tui composer_keys`
- `cargo test -p tui` または少なくとも該当 `multi_pod` / Panel focused tests
- `cargo fmt --check`
- `git diff --check`
- 必要に応じて `cargo check --workspace`

## Related work

- `00001KTNS1AA8` Improve workspace panel display and composer key handling
- `crates/tui/src/composer_keys.rs`
- `crates/tui/src/multi_pod.rs`

## Panel handoff

- workspace: `yoi`
- workspace_orchestrator_pod: `yoi-orchestrator`
