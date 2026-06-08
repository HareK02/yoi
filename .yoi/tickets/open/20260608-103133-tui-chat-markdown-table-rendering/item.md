---
id: '20260608-103133-tui-chat-markdown-table-rendering'
slug: 'tui-chat-markdown-table-rendering'
title: 'TUI chat view should render Markdown tables'
status: 'open'
kind: 'bug'
priority: 'P2'
labels: ['tui', 'chat', 'markdown', 'rendering', 'ux']
workflow_state: 'inprogress'
created_at: '2026-06-08T10:31:33Z'
updated_at: '2026-06-08T13:15:07Z'
assignee: null
risk_flags: ['tui-rendering', 'markdown']
queued_by: 'workspace-panel'
queued_at: '2026-06-08T13:13:23Z'
---

## Background

チャットビューの TUI で、Markdown の Table が描画できない問題がある。会話内の Markdown 出力に表が含まれると、ユーザーが内容を読み取りづらくなり、LLM が表形式で整理した結果や比較情報を TUI 上で確認しにくい。

既存 Ticket を確認した範囲では、同じ目的の open Ticket は見当たらない。関連する過去作業として TUI 会話表示・履歴表示・描画境界の修正 Ticket はあるが、本件は Markdown Table 描画の不具合として独立に扱う。

## Requirements

- TUI の通常のチャット / conversation view で、Markdown の pipe table を読み取り可能に描画できること。
- Assistant 出力に含まれる Markdown Table が、raw text の崩れ・欠落・不正な折り返し・表示不能として扱われないこと。
- 表の描画は TUI の既存のスクロール、折り返し、会話履歴表示、色/スタイル規則と整合すること。
- Markdown Table 非対応が原因で message 全体の描画が壊れないこと。
- 修正は表示層の問題として扱い、Pod history / session log / LLM context の内容は変更しないこと。

## Acceptance criteria

- 少なくとも以下のような Markdown Table を含む assistant message が TUI チャットビューで読み取り可能に表示される。

```markdown
| Item | Status |
| --- | --- |
| A | done |
| B | pending |
```

- Table を含まない既存 Markdown / plain text message の表示が退行しない。
- 幅の狭い terminal でも、表を含む message が panic や行欠落を起こさず、既存の折り返し/切り詰め方針に従って表示される。
- 表の描画ロジックに対する unit / snapshot-like test、または明確な手動確認手順が残っている。

## Binding decisions / invariants

- これは chat view の Markdown 表示不具合であり、Ticket panel / `yoi ticket show` / docs rendering / WebFetch extraction の仕様変更として扱わない。
- 表示のために session history、Pod metadata、worker history、prompt context を書き換えない。
- Markdown Table の完全な GitHub Flavored Markdown 互換性をこの Ticket の必須要件にしない。まず通常の pipe table が読み取り可能に表示されることを優先する。
- 表示不能な入力があっても TUI 全体や message block の描画を壊さず、既存の graceful fallback 方針に従う。

## Implementation latitude

- 既存の Markdown rendering path に table support を追加するか、table を readable plain-text fallback として整形するかは実装者が選んでよい。
- Column width 計算、alignment marker の扱い、cell wrapping の詳細は、既存 TUI の制約と実装コストに合わせて選んでよい。
- 実装調査の結果、Markdown renderer / ratatui widget / line wrapping の責務境界に問題が見つかった場合は、最小の設計整理を含めてよい。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-rendering, markdown]
- open_questions: []

## Escalation conditions

- Table support を入れるために Markdown parser の大きな依存追加や public API 変更が必要になる場合は、実装前に Orchestrator / maintainer に戻す。
- Chat view 以外の TUI surfaces へ広く影響する rendering abstraction 変更が必要になる場合は、範囲拡大として判断を求める。
- GFM 完全互換を目指す必要が出た場合は、本 Ticket の bug fix 範囲を超えるため別 Ticket または設計判断に切り出す。

## Validation

- 関連する TUI / Markdown rendering unit tests を追加または更新する。
- `cargo test` で関連 crate のテストを通す。
- TUI/package/runtime resource に関わる変更のため、完了前に `nix build .#yoi` で package build を確認する。
- 必要なら、Table を含むサンプル message を TUI で表示する手動確認手順を実装報告に残す。

## Related work

- `20260601-013132-tui-new-session-first-message-missing`: TUI 会話ビュー表示の既存修正。
- `20260531-074258-tui-move-view-mode-state`: single-Pod display/history mode の state/render boundary 整理。
- `20260606-060548-workspace-panel-layout-display-tuning`: panel 側の layout/display tuning。対象 surface は異なるため duplicate ではない。
