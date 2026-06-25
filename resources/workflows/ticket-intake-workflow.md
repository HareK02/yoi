---
description: ユーザーの曖昧な依頼を要件同期し、合意済みの Ticket として作成・更新する Intake workflow
model_invokation: true
user_invocable: true
requires: [workflow-resource-boundary]
---
# Ticket Intake Workflow

この bundled workflow は reusable な最小 Intake 手順である。Workspace override は dogfooding 固有の Ticket/Objective/split policy 例を追加してよいが、この workflow の調査ゲート、Ticket 作成前の user agreement、Intake の非 scheduler 境界を弱めてはならない。

Intake の目的は、曖昧な依頼をいきなり実装委譲せず、Orchestrator が routing できる合意済み Ticket または「まだ Ticket 化しない」判断に変換することである。

## 境界

Intake は以下をしない。

- coder / reviewer / read-only investigation helper Worker を起動しない。
- implementation worktree を作らない。
- implementation / review routing、merge、close、branch cleanup をしない。
- unattended scheduler として自動実行しない。
- ユーザー合意なしに official Ticket を作らない。

## Ticket 化前の最小調査ゲート

`TicketCreate` または material な `TicketComment` の前に、必要最小限の調査を行う。

必ず行うこと:

- `TicketList` / `TicketShow` で duplicate / related / blocking-looking work を確認する。
- 既存 Ticket を更新する場合は、その Ticket の item/thread を読む。

次のいずれかに当たる場合は、Ticket 作成前に関連 workflow / prompt / docs / code / config を読む。

- ユーザー依頼が曖昧、または複数の concrete work item を含む。
- 「現在の挙動」「既存仕様」「壊れている」「既にある」など、事実確認を要する claim がある。
- scope / permission / history / prompt context / persistence / public API など authority boundary に触れる。
- 既存 workflow/resource/file の文言変更や source-of-truth 境界に触れる。

調査結果は draft で分けて書く。

- User claims / request snapshot: ユーザーが述べたこと。
- Confirmed facts / sources: Intake が読んで確認したことと source。
- Unverified hypotheses: ありそうだが未確認の推測。
- Undecided points / open questions: ユーザーまたは Orchestrator の判断が必要なこと。

確認できない claim を requirements / acceptance criteria として保存しない。必要な調査が大きい、current-code map がない、または仕様同期が足りない場合は、official Ticket を作らず draft で止め、readiness を `spike_needed` / `requirements_sync_needed` / `blocked` として報告する。

## 手順

1. 依頼を短く言い換え、目的、影響範囲、既決事項、未決定点を分ける。この段階では Ticket を作らない。
2. Ticket 化前の最小調査ゲートを実施する。
3. 要件を同期する。少なくとも observable な完了条件、受け入れ条件、binding decisions / invariants、implementation latitude、validation、escalation conditions を確認する。
4. readiness を分類する。

```text
implementation_ready:
- 意図、受け入れ条件、binding decisions / invariants、implementation latitude、reviewer 判断基準、validation が明確。

requirements_sync_needed:
- 目的は見えているが、仕様・用語・UX・責務境界・受け入れ条件が未同期。

spike_needed:
- 技術調査、依存関係、性能、license、diagnostics、現在コード map が先に必要。

blocked:
- 人間判断、外部イベント、別 Ticket の完了が必要。
```

5. 作成前 draft を提示する。

```text
Title:
Priority:
Readiness:
Next Ticket operation: draft_only | create_after_user_agreement | update_existing_after_user_agreement | no_ticket
Risk flags:

User claims / request snapshot:
Confirmed facts / sources:
Unverified hypotheses:
Undecided points / open questions:

Background:
Requirements:
Acceptance criteria:
Binding decisions / invariants:
Implementation latitude:
Escalation conditions:
Validation:
Related tickets/docs/files:
```

6. ユーザーの明示承認、または「作って」「切って」「記録して」など official record 作成の明示指示を待つ。
7. 合意後だけ `TicketCreate` / `TicketComment` を使う。canonical ID は storage が割り当てるため draft では提案しない。
8. 作成/更新後は id/title、readiness、open questions/risk flags、次の Orchestrator routing 候補を報告して止まる。

## 推奨 Ticket body

```markdown
## User claims / request snapshot

## Confirmed facts / sources

## Unverified hypotheses

## Undecided points / open questions

## Background

## Requirements

## Acceptance criteria

## Binding decisions / invariants

## Implementation latitude

## Readiness

- readiness: implementation_ready | requirements_sync_needed | spike_needed | blocked | unspecified
- risk_flags: [...]

## Escalation conditions

## Validation

## Related work
```
