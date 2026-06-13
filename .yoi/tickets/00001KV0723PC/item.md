---
title: 'Panel Quit 時の断続的な遅延を調査して解消する'
state: 'queued'
created_at: '2026-06-13T10:04:55Z'
updated_at: '2026-06-13T10:53:17Z'
assignee: null
readiness: 'spike_needed'
risk_flags: ['tui-panel', 'shutdown-latency', 'async-cancellation']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T10:53:17Z'
---

## Background

Panel から Quit するときに、終了まで遅延が発生することがある。ユーザー観測の断続的な UX 劣化として扱い、原因を調査したうえで修正する。

関連しそうな既存記録として、Panel の非同期遷移/フリーズ回避に関する `00001KTFMMZP0` は closed。今回の主対象は Quit 操作の遅延であり、同一目的の未完了 Ticket は確認できなかった。

## Request snapshot

- 依頼: 「PanelからQuitするときに遅延が発生することがある。調査して修正チケット切って」
- handoff workspace: `yoi`
- workspace_orchestrator_pod: `yoi-orchestrator`

## Requirements

- `yoi panel` / workspace Panel の Quit 操作で、ユーザー入力後に不必要な待ちが発生する原因を特定する。
- Quit は、通常の端末復旧や安全な最小限の cleanup を除き、進行中の reload / notice dispatch / snapshot 読み込み / Pod 状態観測などの完了待ちでブロックされないこと。
- 断続的な遅延であっても、原因が再発しにくい形で修正する。単に poll interval を短くするだけの対症療法にしない。
- 既存の Panel 役割、Ticket 操作、Companion/Orchestrator composer target、row selection semantics を壊さない。

## Acceptance criteria

- Quit 入力（現状の `Ctrl+C` / `Ctrl+D` 経路を含む）が、Panel の background reload や queue-attention notice などの非本質的処理待ちで目に見えて遅延しない。
- 遅延原因と修正方針が implementation report に説明されている。
- 可能な範囲で unit test または小さな regression test が追加され、少なくとも Quit 経路が pending background work によってブロックされないことを検証する。
- 自動化が難しい場合は、manual validation 手順と観測結果を implementation report に残す。

## Binding decisions / invariants

- Quit はユーザーの明示的な終了意思であり、Panel の観測/reload/通知送信の完了を待つために遅延してはならない。
- ただし端末状態復旧、描画 backend の正常終了、Rust drop による安全な abort など、最小限の終了処理は維持する。
- Quit 改善のために Ticket lifecycle authority、Pod authority boundary、Panel row/action semantics を変更しない。
- `resources/prompts` や durable Ticket schema の変更を伴う必要は、現時点では想定しない。必要になった場合は escalation する。

## Implementation latitude

- まず `crates/tui/src/multi_pod.rs` の Panel event loop / `PendingReload` / Quit handling 周辺を調査する。
- 必要に応じて quit 受付前後の await、background task abort/drop、terminal event polling、queue-attention notice dispatch、snapshot load の相互作用を確認する。
- 修正手段は実装者に委ねるが、終了時に不要な await を避ける、Quit 前に cancellable background work を abort する、または event loop の優先度を調整するなど、設計上説明可能な変更にする。
- 再現が難しい場合は、遅延し得るコードパスを単体で再現できる test seam を作ることを優先してよい。

## Readiness

- readiness: spike_needed
- 理由: 現象は明確だが断続的であり、現時点では再現条件・遅延箇所・影響する async path が未特定。先に短い調査/計測または code-path analysis が必要。
- risk_flags: [tui-panel, shutdown-latency, async-cancellation]

## Open questions

- 遅延の典型時間、発生頻度、再現しやすい Panel 状態（reload 中、Orchestrator notice 中、Pod 多数、dirty workspace 等）は未提供。実装者は必要なら調査中に記録する。

## Escalation conditions

- 修正に Panel の public UX、Ticket lifecycle semantics、Pod shutdown semantics、authority boundary の変更が必要になりそうな場合は Orchestrator/maintainer に戻す。
- 端末 cleanup や Pod process lifecycle を犠牲にしないと遅延を解消できない場合は、方針判断を求める。
- 遅延の原因が Panel 外（OS 端末、shell、external command、specific provider/network）にある場合は、証拠と切り分け結果を残して routing し直す。

## Validation

- 変更内容に応じて `cargo test -p yoi-tui` または該当 crate の focused test を実行する。
- 必要に応じて `cargo check` / `git diff --check` を実行する。
- 可能なら `yoi panel` を実際に起動し、background reload があり得る状態でも Quit が速やかに戻ることを手動確認する。

## Related work

- `00001KTFMMZP0`: Panel の非同期遷移/フリーズ回避に関する closed Ticket。今回の修正で既存判断と矛盾しないか参考にする。
