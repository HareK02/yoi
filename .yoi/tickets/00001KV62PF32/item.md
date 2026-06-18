---
title: 'Panel startup latency E2E を一覧データ描画完了基準に修正する'
state: 'inprogress'
created_at: '2026-06-15T16:44:06Z'
updated_at: '2026-06-18T12:05:59Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'e2e', 'startup-latency', 'readiness-metric', 'ticket-list-rendering']
queued_by: 'workspace-panel'
queued_at: '2026-06-17T09:46:03Z'
---

## Background

既存の Panel startup latency E2E は、`panel_ready` を主要な startup readiness として扱っている。しかし現在の `panel_ready` は `terminal.draw(|f| draw(f, app))` の初回完了直後に emit される first visible frame であり、Ticket / Pod / Orchestrator 一覧データが取得され、Panel rows として実際に描画されたことを意味しない。

ユーザーが問題にしている startup latency は、空または loading の初回 frame ではなく、Panel が実用可能な一覧データを表示するまでの時間である。したがって、既存 E2E の前提は不十分であり、startup latency E2E の readiness 基準を「一覧データ取得後の row render」に修正する必要がある。

この Ticket は、first frame の高速化を否定するものではない。first frame metric は補助 metric として残してよい。ただし startup latency の主 assertion / 報告値は、fixture の期待 Ticket / Pod / Orchestrator rows が ViewModel に反映され、描画後の row metadata / screen output として観測できたタイミングを基準にする。

## Requirements

- Panel startup latency E2E の readiness 定義を明確化する。
  - `panel_first_frame`: 初回 visible draw。loading / empty frame でもよい。
  - `panel_rows_ready` または同等: 一覧データが取得され、期待 row が描画された状態。
- Startup latency の主 budget / assertion は `panel_rows_ready` 相当に置く。
  - `panel_ready` / first frame を「一覧 ready」として扱わない。
- Fixture は期待される一覧データを具体的に持つ。
  - 少なくとも fixture Ticket id / title / state の row が描画されたことを assert する。
  - 必要に応じて Pod row / orchestration overlay row も fixture に含める。
- `rows_rendered.len() >= N` のような弱い条件だけに依存しない。
  - 期待 Ticket id / title / state / row kind など、実データ反映を確認する。
- E2E event naming / diagnostics を誤解しにくくする。
  - `panel_ready` が残る場合は first frame として説明する。
  - 一覧 ready 用の別 event / helper / metric 名を追加する。
- Background reload / observation が遅い場合でも、first frame と rows ready を別々に測る。
  - first frame が速いことと、一覧 ready が速いことを混同しない。
- Existing Panel behavior を変更する場合は、loading frame 後に reload を開始する設計を維持してよいが、latency E2E の成功条件を loading frame だけで満たさないようにする。
- E2E report / Ticket implementation report では、before/after 数値が何を測っているかを明記する。
  - first visible frame
  - fixture rows ready
  - full/background observation completion if measured

## Acceptance criteria

- Panel startup latency E2E が、fixture の具体的な Ticket row が描画されたタイミングを readiness として測定する。
- `panel_ready` / first visible frame だけでは startup latency E2E の主 assertion が通らない。
- Fixture row readiness は `len >= 2` などの抽象条件ではなく、期待 Ticket id / title / state 等で確認される。
- E2E output / helper 名 / comments から、first frame と rows ready の違いが分かる。
- Background reload を hold するテストがある場合、first frame は先に出てもよいが、rows ready はデータ反映まで完了しないことを確認できる。
- Existing Panel mouse / row selection E2E に regression がない。
- Tests cover:
  - first frame event does not imply list readiness
  - fixture Ticket row rendered readiness
  - delayed reload delays rows-ready metric but not necessarily first-frame metric
  - row readiness fails when expected fixture row is absent
- Validation: `cargo test -p yoi-e2e --features e2e panel`, relevant `cargo check`, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` if runtime/package/dependency changes occur.

## Non-goals

- Solving all live workspace startup latency causes in this Ticket.
- Replacing Panel ViewModel architecture.
- Requiring every Pod socket / Orchestrator observation to fully settle before any UI is shown.
- Removing first visible frame metric entirely.
- Changing Ticket lifecycle semantics.

## Related work

- `00001KV5MRH6D` — Panel startup latency E2E / first visible frame separation work.
- `00001KV5D7MG5` — Panel orchestration worktree Ticket state overlay.
- `00001KV4ZPAD3` — Panel Ticket / Intake Pod row visual hierarchy.
