---
title: 'Panel: invalid Ticket があっても Ticket 機能全体を無効化しない'
state: 'inprogress'
created_at: '2026-06-14T14:56:51Z'
updated_at: '2026-06-14T15:57:47Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-panel', 'ticket-backend', 'partial-failure', 'diagnostics']
queued_by: 'workspace-panel'
queued_at: '2026-06-14T15:35:56Z'
---

## Background

`yoi panel` は Ticket-enabled workspace の主要操作面である。invalid / corrupt / unreadable な Ticket record が1件あるだけで、他の正常な Ticket の表示・Queue・Intake / Orchestrator 導線まで巻き添えで失われると、復旧作業と通常運用が難しくなる。

ユーザー依頼:

> InvalidなTicketがあった場合にPanelのチケット機能全般が無効化されるのではなく、

Intake で関連コードを確認したところ、`crates/tui/src/workspace_panel.rs` の `build_ticket_rows` は次のように全体 `Result<Vec<PanelRow>>` になっている。

- `backend.list(TicketFilter::all())?` が失敗すると Ticket rows 全体が失敗する。
- 各 summary に対する `backend.show(TicketIdOrSlug::Query(summary.id.clone()))?` が1件でも失敗すると Ticket rows 全体が失敗する。
- 呼び出し側はその error を `Ticket rows unavailable: ...` diagnostic として扱い、正常 Ticket rows も表示されない。

したがって、本件は個別 Ticket record の partial failure handling として concrete bugfix にできる。

## Requirements

- Workspace Panel は invalid / corrupt / unreadable な Ticket record が一部にあっても、正常な Ticket rows を表示し続ける。
- 正常な Ticket に対する既存の Panel action を維持する。
  - planning clarification / Intake launch
  - ready Ticket の Queue
  - queued / inprogress / done / closed など既存 state 表示
  - Orchestrator / Companion 関連の既存導線
- invalid Ticket の存在は隠さない。
  - header diagnostic、専用 placeholder row、または bounded detail で、どの Ticket/path が問題か分かるようにする。
  - raw secret-like content や巨大ログは表示しない。
- invalid Ticket には危険な lifecycle action を出さない。
  - Queue / Close / planning return などの action は、正常に読めた Ticket に限定する。
- Ticket config 自体が unusable な場合は、従来通り Ticket 機能を使えない状態としてよい。
  - 本件は「Ticket backend/config 全体が壊れている」ケースではなく、「個別 Ticket record が壊れている」ケースの partial failure handling。
- `TicketDoctor` / backend diagnostics と意味がずれないようにする。
  - Panel 独自の silent skip ではなく、ユーザーが修復すべき record を認識できる。

## Acceptance criteria

- 1件の invalid Ticket record がある状態でも、Panel は正常な Ticket rows を表示する。
- 正常な ready Ticket では Queue action が引き続き利用できる。
- 正常な planning Ticket では clarification / Intake 導線が引き続き利用できる。
- invalid Ticket は bounded diagnostic または placeholder row として見える。
- invalid Ticket に対して Queue / Close / lifecycle mutation などの action は提示されない。
- Panel header / diagnostics は「Ticket 全体 unavailable」ではなく「一部 Ticket を読み込めなかった」ことを示す。
- Ticket config が unreadable / backend root が unusable な場合の既存 degraded behavior は壊さない。
- Focused tests で、少なくとも次を確認する。
  - valid + invalid が混在する Ticket backend で valid rows が残る。
  - invalid row/diagnostic が bounded に出る。
  - valid Ticket の action が維持される。
  - config unusable ケースは従来通り全体 unavailable。

## Binding decisions / invariants

- invalid Ticket を理由に、正常 Ticket の Panel 操作を巻き添えで止めない。
- invalid Ticket record を Panel が自動修復・自動削除しない。
- invalid Ticket に対して lifecycle mutation action を出さない。
- Ticket lifecycle authority / state schema は変更しない。
- Ticket backend config 全体が unusable な場合と、個別 record の partial failure は区別する。
- 正常 Ticket の lifecycle mutation は、既存の typed Ticket backend / Panel action path を通す。

## Implementation latitude

- invalid Ticket の表示方法は実装側に裁量を残す。
  - header diagnostic のみ
  - disabled/error placeholder row
  - diagnostics detail view / F2 detail への誘導
- `LocalTicketBackend::list` を lossy にするか、Panel 側で per-Ticket load を recover するかは実装側判断でよい。
- backend に partial diagnostic API を足す必要がある場合は、Panel 専用の ad hoc parsing ではなく typed boundary を保つ。
- `TicketDoctor` の診断ロジックを再利用できるならよいが、Panel 起動ごとに重すぎる full doctor を必須にしない。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-panel, ticket-backend, partial-failure, diagnostics]

## Escalation conditions

- `TicketBackend::list` の public semantics を変更しないと実現できない場合。
- invalid Ticket の path / id を安全に特定できず、diagnostic 表示の責務境界が曖昧な場合。
- Panel action dispatch が row identity を valid Ticket と invalid placeholder で安全に分けられない場合。
- `TicketDoctor` と Panel diagnostics の severity / wording が矛盾する場合。
- invalid Ticket の内容を読まないと UI 表示できない設計になり、secret-like content 露出リスクが出る場合。

## Validation

- `cargo test -p tui workspace_panel --lib`
- 必要に応じて `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`

## Related work

- `00001KV12W2RT` — Panel Ticket rows を2行表示にして gate 情報を分離する
- `00001KV09WYC6` — Workspace panel: show Ticket-associated Intake Pods adjacent to Ticket rows
- Code areas:
  - `crates/tui/src/workspace_panel.rs`
  - `crates/tui/src/multi_pod.rs`
  - `crates/ticket/src/lib.rs`
