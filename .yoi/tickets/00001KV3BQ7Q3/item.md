---
title: '対象 TUI/Panel merge commit の挙動を現行 E2E で確認する'
state: 'closed'
created_at: '2026-06-14T15:24:05Z'
updated_at: '2026-06-15T06:33:44Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['e2e', 'tui', 'panel', 'regression-evidence']
queued_by: 'workspace-panel'
queued_at: '2026-06-14T15:35:57Z'
---

## Background

過去に以下の merge commit が、user-visible な TUI / Panel 挙動として十分な実経路確認なしに merge された、または merge 後に証明不足が問題化した。

- `802fa1f00f8725fe35336e083cd05652fee1409e` — `merge: rewind live refresh`
- `02311883f7cda116676d8e179a14ad0be9e7a244` — `merge: panel mouse selection`
- `db7bad7a64766c2039a4c10781801cb571027955` — `merge: panel quit latency`

特に Panel latency / mouse selection は、focused tests や code review だけでは実ユーザー経路の保証として不十分だった経緯がある。後続で E2E harness が整備されたため、現行の E2E システムで、これらの commit が意図した user-visible behavior を確認する。

この Ticket は「過去の merge 判断を後付けで正当化する」ためではなく、現行 HEAD / 現行 E2E infrastructure において、対象 behavior が実プロセス PTY 経路で確認できるかを明示するための validation work item である。

## Requirements

- 対象 commit ごとに、確認すべき user-visible behavior を明確化する。
  - `802fa1f0`: rewind picker で `Enter` 後、live TUI 表示が restart / restore なしに巻き戻し後状態へ更新されること。
  - `02311883`: Panel mouse selection が実プロセス PTY 経路で動作し、click が selection を変え、click だけで action dispatch しないこと。
  - `db7bad7a`: Panel quit latency 改善として意図された経路が、現行 E2E で bounded に確認できること。少なくとも pending background work が quit を user-visible に block しないことを確認し、元の latency 観測を直接保証できない場合はその gap を明示する。
- 現行 E2E システムで各 behavior を確認する。
- 既存 E2E coverage がある場合は、どの test / scenario がどの commit の behavior を確認しているかを実装報告に明記する。
- 既存 E2E coverage が不足している場合は、最小限の E2E scenario / assertion を追加または更新して確認可能にする。
- E2E で確認できない性質がある場合は、pass 扱いにせず、coverage gap として明示する。
- 実ユーザー環境での manual/live confirmation と、fixture PTY E2E confirmation を混同しない。

## Acceptance criteria

- `802fa1f0`, `02311883`, `db7bad7a` の3件それぞれについて、現行 E2E での確認結果が pass / fail / coverage gap のいずれかとして記録されている。
- pass とする場合は、対応する E2E test 名、assertion、実行 command、結果が実装報告に記録されている。
- coverage gap とする場合は、何が現行 E2E では確認できないのか、追加 E2E が必要なのか、manual/live validation が必要なのかが明示されている。
- mouse selection は、focused unit test ではなく実 `yoi` binary + PTY 経路で `selection_changed` 等の user-visible observer を確認している。
- quit latency は、単に process が終了するだけでなく、latency / pending work / threshold の観点で何を保証しているかが明示されている。
- rewind live refresh は、restart / restore なしの live 表示更新が E2E で確認されるか、未確認ならその不足が明示されている。
- `cargo test -p yoi-e2e --features e2e` または同等の現行 E2E command が実行され、結果が記録されている。
- E2E を追加・更新した場合、fixture-local HOME / XDG / runtime / workspace isolation と no-provider / no-network 前提を維持している。

## Binding decisions / invariants

- focused tests / code-path review だけで user-visible Panel/TUI behavior を確認済み扱いにしない。
- E2E pass と manual/live user confirmation は別物として記録する。
- 現行 E2E が確認していない behavior を、確認済みとして表現しない。
- この Ticket の主目的は validation / evidence 整理であり、対象 behavior の大きな再設計や unrelated fix は行わない。
- 対象 commit の historical merge decision を書き換えるのではなく、現行状態の evidence を追加する。

## Implementation latitude

- 既存 `yoi-e2e` scenario の再利用、test 名 / assertion の明確化、必要最小限の scenario 追加は実装者判断で行ってよい。
- E2E observer / fixture helper に不足がある場合、production behavior に影響しない `e2e-test` feature gate 配下の補助を追加してよい。
- latency threshold は既存 E2E の基準を優先し、変更が必要な場合は理由を実装報告に明記する。
- 既存 E2E が flake する場合は、原因を切り分け、test hardening と behavior fix を混同しない。

## Readiness

- readiness: implementation_ready
- risk_flags: [e2e, tui, panel, regression-evidence]

## Escalation conditions

- 現行 E2E infrastructure では対象 behavior を原理的に確認できず、実端末 manual validation や新しい harness 設計が必要な場合。
- latency の再現・測定が既存 observer / threshold では意味を持たない場合。
- mouse behavior が PTY SGR injection では pass するが、実端末 mouse 操作と乖離する疑いが残る場合。
- 対象 behavior の確認中に actual regression が見つかり、validation ticket の範囲を超える修正が必要になった場合。

## Validation

- `cargo test -p yoi-e2e --features e2e`
- 必要に応じて対象 E2E scenario の narrow run
- E2E 追加・更新時:
  - `cargo test -p yoi-e2e --no-run`
  - `cargo fmt --check`
  - `git diff --check`
- 変更範囲に応じて `cargo check -p yoi-e2e -p yoi -p tui`

## Related work

- `00001KV04NJ8D` — TUI rewind picker の Enter 後に live 表示が巻き戻らない問題を調査・修正する
- `00001KV072V89` — Panel mouse selection
- `00001KV0723PC` — Panel quit latency
- `00001KSKBP9YG` — opt-in Panel PTY E2E harness
- `00001KV0TJVN5` — E2E binary provider / latest binary validation
- `00001KV0YK5S0` — E2E tmp/runtime isolation
- `00001KV10SN02` — E2E critical path coverage
