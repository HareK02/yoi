---
title: 'TUI Console: Paused 中の Ctrl+X で中断ターンをキャンセルできるようにする'
state: 'inprogress'
created_at: '2026-06-23T07:02:07Z'
updated_at: '2026-06-23T13:37:51Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-keybinding', 'pod-lifecycle']
queued_by: 'workspace-panel'
queued_at: '2026-06-23T08:41:00Z'
---

## User claims / request snapshot

- TUI Console で、`Ctrl+C` により Pause している状態のとき、`Ctrl+X` でキャンセルできるようにしたい。

## Confirmed facts / sources

- 既存 Ticket:
  - 未完了 Ticket に直接 duplicate は確認できなかった。
  - `00001KSVP63K8 Support immediate in-flight TUI composer injection` は TUI 実行中入力に関連するが、「Do not interrupt/cancel the current run」が non-goal なので今回の要求とは別件。
- `crates/tui/src/console/mod.rs`
  - 現在の `Ctrl+X` は `PodStatus::Running` のときだけ `Method::Cancel` を返す。
  - `PodStatus::Paused | PodStatus::Idle` のときは `Method::Shutdown` を返している。
  - `Ctrl+C` は Running では `Method::Pause`、Idle/Paused では TUI 終了の 2-tap guard として扱われている。
  - 既存 test `pause_and_cancel_clear_queued_input` は名前上 pause/cancel を扱うが、`Ctrl+C` 後に app status を Paused に更新していないため、Paused 状態での `Ctrl+X` cancel は実質カバーしていない。
- `crates/protocol/src/lib.rs`
  - `Method::Pause` は in-flight turn を止めて Paused に遷移するもの。
  - コメント上、`Cancel` は破棄して Idle に戻る意味を持ち、Paused は `Resume` または新規 `Run` が可能。
- `crates/pod/src/controller.rs`
  - 非 running 側の `Method::Cancel` は現状 `NotRunning` error を返す経路があるため、TUI keymap 変更だけでなく Paused 状態の cancel semantics を確認・必要なら実装する必要がある。

## Unverified hypotheses

- 実装は比較的小さい可能性が高いが、`Paused` 状態で `Method::Cancel` を受け取った時に、Worker/Pod の interrupted state や orphan tool-use cleanup をどう clear するべきかは実装時に確認が必要。
- UI 表示は paused hint に `Ctrl+X to cancel` 相当を追加するだけで足りる可能性がある。

## Undecided points / open questions

- blocking な未決定点はなし。
- Orchestrator/実装者判断で、`Ctrl+X` の Idle 挙動を現状通り `Shutdown` に残すか、Paused のみ `Cancel` に分岐するのがよさそう。

## Background

`Ctrl+C` で Pause した turn は resume 可能だが、ユーザーが「やっぱりこの中断 turn は破棄したい」と判断した時に、Console 上の自然な操作として `Ctrl+X` cancel が効くべき。現在は Paused/Idle の `Ctrl+X` が shutdown に割り当てられており、Paused turn のキャンセル導線として機能していない。

## Requirements

- TUI Console で Pod が `Paused` のとき、`Ctrl+X` は shutdown ではなく paused/interrupted turn の cancel を要求する。
- `Running` 中の `Ctrl+X -> Method::Cancel` は維持する。
- `Idle` 中の `Ctrl+X` の挙動は、意図的に変更する理由がなければ現状の shutdown を維持する。
- Paused 状態で cancel した後、Pod は resume 可能な paused turn を残さず、Idle 相当として次の入力を受けられる。
- queued input がある場合は、既存 Running cancel と同様に誤送信されないよう clear される。
- UI の paused hint / actionbar / tests など、ユーザーが操作を理解できる範囲を必要に応じて更新する。

## Acceptance criteria

- `Ctrl+C` で Running turn を Pause した後、Console 上で `Ctrl+X` を押すと、TUI 終了や Pod shutdown ではなく turn cancel が実行される。
- Paused cancel 後、`Enter` で前の turn を resume しようとしても resume されない。
- Paused cancel 後、新規入力は通常の新規 turn として扱われる。
- Running 中の `Ctrl+X` cancel と Idle 中の `Ctrl+X` 挙動に意図しない regressions がない。
- Focused tests が Paused 状態の `Ctrl+X` key handling と Pod/controller 側の cancel semantics をカバーする。

## Binding decisions / invariants

- `Ctrl+C` の Pause semantics は変更しない。
- `Ctrl+C` の Idle/Paused 2-tap TUI quit guard は、この Ticket では置き換えない。
- この Ticket は in-flight input injection / history append policy を扱わない。
- provider stream を直接 mutate しない。
- cancel は hidden context/history mutation ではなく、既存の typed `Method::Cancel` semantics またはそれに準じる明示的 lifecycle 操作で表現する。

## Implementation latitude

- `Paused` 中の `Ctrl+X` を `Method::Cancel` に変えるだけで足りるか、controller/worker 側に Paused cancel handling を追加するかは実装調査で判断してよい。
- UI hint の文言や配置は実装者に任せてよい。
- 必要なら既存 test を修正・分割して、「Running cancel」と「Paused cancel」を明確に分ける。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-keybinding, pod-lifecycle]

## Escalation conditions

- `Method::Cancel` を Paused 状態に拡張すると session/history cleanup や orphan tool-result closure の設計判断が必要になる場合は、実装者は Orchestrator/maintainer に戻す。
- `Ctrl+X` の Idle 挙動変更が必要になる場合は、別途合意を取る。

## Validation

- Focused tests:
  - `cargo test -p tui pause` または該当 TUI key handling tests
  - `cargo test -p pod cancel` または controller paused cancel の focused tests
- 通常確認:
  - `cargo fmt --check`
  - 必要に応じて `cargo check -q`
  - terminal UX 変更なので、可能なら実機 TUI/manual check も推奨。

## Related work

- Related, not duplicate: `.yoi/tickets/00001KSVP63K8`
- Files:
  - `crates/tui/src/console/mod.rs`
  - `crates/tui/src/ui.rs`
  - `crates/protocol/src/lib.rs`
  - `crates/pod/src/controller.rs`
