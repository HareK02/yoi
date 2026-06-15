---
title: 'Panel 起動遅延の待ち要因を E2E 計測で特定し改善する'
state: 'queued'
created_at: '2026-06-15T12:40:33Z'
updated_at: '2026-06-15T13:59:47Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'tui', 'e2e', 'latency', 'runtime-observation']
queued_by: 'workspace-panel'
queued_at: '2026-06-15T13:59:47Z'
---

## Background

`yoi panel` 起動時に最大 7 秒程度かかることがある。TUI が表示されている裏で具体的に何を待っていて遅いのかを特定し、改善したい。さらに E2E で起動時間を測定し、E2E テストとして起動時間の基準を設けたい。

過去に `00001KTFMMZP0` で Panel transition / first draw の非同期化を実施済みだが、現在も startup latency が観測されている。`00001KV3BQ7Q3` では Panel/TUI の user-visible behavior を現行 E2E で確認済みだが、今回の startup latency の分解・改善・基準化は別の concrete work item として扱う。

## Requirements

- `yoi panel` 起動時の startup path を計測し、どの処理をどの順序で待っているかを特定する。
  - 例: snapshot load、Ticket backend scan、Pod metadata/socket observation、Orchestrator/Companion observation、background reload barrier、terminal setup、first draw、E2E observer setup など。
- TUI が表示されている裏で待っている処理と、初回表示前に同期的に待っている処理を区別する。
- 実ユーザーに影響する起動 latency を改善する。
- E2E で `yoi panel` 起動時間を測定できる scenario / helper / observer を追加または更新する。
- E2E テストに、少なくとも以下のような基準を設ける。
  - first visible frame / initial panel render が bounded に出ること。
  - 必要なら full ready / background reload complete も別 metric として測る。
- 「最大 7 秒程度」の現象が E2E fixture で再現しない場合も、何を保証できたか、何が live/manual gap として残るかを明示する。
- 改善後の実装報告に、計測結果 before / after、待ち要因、変更点、残る gap を記録する。

## Acceptance criteria

- `yoi panel` startup E2E が実 `yoi` binary + PTY 経路で起動時間を測定している。
- E2E は単に process が起動することだけでなく、少なくとも **初回 visible panel/render 到達までの時間** を assertion している。
- 起動時間 budget が E2E test に明示されている。
  - 提案値: fixture PTY 上の first visible panel/render は `1500 ms` 以内。
  - full ready / background reload complete を測る場合は、別 budget として設定し、first visible budget と混同しない。
- 既存環境差・CI/fixture のばらつきに対して、過度に flaky でない threshold / retry / observer 設計になっている。
- 実装報告に以下が記録されている。
  - 計測対象 command / test name。
  - before / after の測定結果。
  - startup path の主要 wait point。
  - どの wait を削減・非同期化・遅延実行したか。
  - E2E が保証する範囲と、保証しない live/manual gap。
- 既存 Panel/TUI E2E と fixture-local HOME / XDG / runtime / workspace isolation を壊さない。
- no-provider / no-network 前提を維持する。
- 既存の Ticket workflow / Pod restore/spawn authority / Orchestrator queue semantics を変更しない。

## Binding decisions / invariants

- focused/unit test やコードレビューだけで startup latency 改善を確認済み扱いにしない。
- E2E pass と manual/live terminal confirmation を混同しない。
- `first visible render` と `all background work complete` を同じ metric として扱わない。
- 起動を速く見せるために、Ticket state / Pod state / Orchestrator state の authority を偽って表示しない。
- background reload / observation は完了後に正しい state / diagnostics を反映する。
- 起動時間計測のために provider/network/secret 依存を導入しない。
- broad TUI runtime rewrite や scheduler/lease 導入は今回の前提にしない。必要になった場合は escalation する。

## Implementation latitude

- 計測 instrumentation の入れ方は実装者判断でよい。
  - E2E-only observer / event。
  - `e2e-test` feature gate 配下の timing marker。
  - PTY output marker。
  - structured diagnostic event。
- exact wait point の分解方法は実装者判断でよいが、実装報告で説明可能にする。
- threshold はまず `first visible render <= 1500 ms` を提案値とする。
  - 実測上、fixture 環境で妥当でない場合は、実装報告で理由を示して調整してよい。
  - ただし 7 秒級の regression を許す threshold にはしない。
- startup latency 改善は、既存の `PendingReload` / background observation / loading state を活かしてよい。
- full ready までの時間が本質的に長い場合は、first visible と full ready を分けた上で、full ready の wait reason を表示・記録する改善でもよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [panel, tui, e2e, latency, runtime-observation]

## Escalation conditions

- 7 秒 latency が E2E fixture では再現せず、live terminal / 実 workspace 固有の manual validation が必要な場合。
- 起動遅延の主因が Ticket storage corruption、大量 Pod metadata、外部 provider、filesystem stalls など fixture 外条件に依存する場合。
- startup path の改善に Pod authority / Ticket workflow semantics / Orchestrator lifecycle semantics の変更が必要になる場合。
- stable な timing test を作るには現行 E2E harness の設計変更が必要な場合。
- threshold を設けると flaky になり、別の observer / benchmark style が必要と判断される場合。

## Validation

- `cargo test -p yoi-e2e --features e2e`
- 必要に応じて対象 E2E scenario の narrow run
- E2E 追加・更新時:
  - `cargo test -p yoi-e2e --features e2e --no-run`
  - `cargo fmt --check`
  - `git diff --check`
- 変更範囲に応じて:
  - `cargo check -p yoi-e2e -p yoi -p tui`
  - `cargo test -p tui` または focused panel/tui tests

## Related work

- `00001KTFMMZP0` — `Make workspace panel transitions non-blocking`
- `00001KV3BQ7Q3` — `対象 TUI/Panel merge commit の挙動を現行 E2E で確認する`
- Panel handoff context:
  - workspace: `yoi`
  - workspace_orchestrator_pod: `yoi-orchestrator`
