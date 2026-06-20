---
title: 'Intake workflow に Ticket 化前の調査ゲートを明示する'
state: 'closed'
created_at: '2026-06-20T11:45:00Z'
updated_at: '2026-06-20T12:20:16Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['prompt-context', 'workflow-source', 'role-behavior', 'ticket-authority']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T12:06:37Z'
---

## Background

Intake がユーザー発話をそのまま Ticket 化しようとし、Ticket 作成前に必要な既存 Ticket / docs / code / workflow 調査を十分に行わない挙動が観測されている。

今回の Intake 調査では、workspace 側の `.yoi/workflow/ticket-intake-workflow.md` には既存 Ticket 確認、関連 docs/code/workflow/history の確認、readiness 分類、ユーザー合意前に official Ticket を作らないことが書かれている一方で、以下の弱さが見えた。

- `resources/prompts/role/intake.md` は role prompt として短く、`create or update the appropriate Ticket` の重みが強い。
- `resources/workflows/ticket-intake-workflow.md` は workspace workflow snapshot より薄く、Ticket 化前の調査ゲート、draft-before-create、user agreement gate、`spike_needed` の扱いが弱い。
- workspace workflow でも「必要に応じて関連 docs / code / workflow / history を読む」が optional に読めるため、曖昧な依頼で調査せず draft/Ticket 化へ倒れやすい。
- draft template に `Action required` / `Attention required` が残っており、現在の Ticket schema / vocabulary とずれた文言が残っている。

関連して参照した既存 Ticket:

- `00001KTAZ2401` Ticket intake workflow
- `00001KT0JPZS0` Built-in Ticket intake and orchestration routing
- `00001KTRKZ14C` Project workflows を public builtin と dogfood 運用に分離する
- `00001KSKBPHRG` Prompt / Workflow 評価メトリクスと改善 Offer

## Requirements

- Intake role prompt に「Ticket 化前の最小調査」を明示する。
- Ticket intake workflow に、以下を binding step として追加または強調する。
  - 既存 Ticket / workflow / relevant files を読むべき条件。
  - 調査不足なら `TicketCreate` せず、draft または `spike_needed` / `requirements_sync_needed` として止めること。
  - ユーザー主張、Intake が確認した事実、未確認仮説を Ticket draft 上で分けること。
  - “言われたことそのまま” を requirements / acceptance criteria にしないこと。
- `resources/workflows/ticket-intake-workflow.md` と workspace workflow `.yoi/workflow/ticket-intake-workflow.md` の役割差を確認し、必要なら bundled 側も詳細化する。
- stale な `Action required` / `Attention required` 語彙を削除または現在の Ticket 運用に合う表現へ置換する。
- Intake が coder / reviewer / read-only investigation helper Pod を起動しない境界は維持する。

## Acceptance criteria

- Intake が曖昧な依頼を受けた時、`TicketCreate` より先に duplicate / related work / relevant docs-code-workflow の確認を行うべき条件が model-facing prompt/workflow に明文化されている。
- 調査が必要な依頼では、Intake が `spike_needed` または `requirements_sync_needed` として draft 提示に留められることが prompt/workflow 上で明示されている。
- role prompt が “create/update Ticket” だけでなく “materialize 前に十分に調査し、未確認事項を分離する” ことを明示している。
- bundled workflow resource と workspace workflow の不整合が解消されるか、意図した差分として短く説明されている。
- Ticket 作成前の user agreement rule は維持されている。
- stale な `Action required` / `Attention required` が新規 draft template から消えるか、現行 schema と矛盾しない説明に置き換わっている。

## Binding decisions / invariants

- Intake は scheduler ではなく、coder / reviewer / read-only investigation helper Pod を起動しない。
- Intake は implementation worktree 作成、implementation routing、review routing、merge、close を行わない。
- ユーザー合意なしに official Ticket を作らないルールは維持する。
- Ticket body には、ユーザー主張、Intake が確認した事実、未確認仮説、未決定点を混同して保存しない。
- Prompt / workflow 文言は `resources/prompts` / `resources/workflows` と workspace workflow override の責務境界を崩さない。

## Implementation latitude

- prompt / workflow 文言の修正で足りるならコード変更しない。
- 実例セッションが必要なら、`~/.yoi/sessions` の該当 transcript を小さく確認して原因分析に使ってよい。ただし raw private context や不要な transcript 全文を Ticket に保存しない。
- `00001KSKBPHRG` の prompt/workflow evaluation work と接続して、将来的な評価シナリオにするのは可。
- bundled workflow と dogfood workspace workflow を完全一致させる必要はないが、差分がある場合は意図を説明できる状態にする。

## Readiness

- readiness: implementation_ready
- risk_flags: [prompt-context, workflow-source, role-behavior, ticket-authority]

## Escalation conditions

- Intake の観測挙動が prompt/workflow ではなく Panel handoff、workflow selection、role profile resolution、または active workflow snapshot の問題に見える場合。
- workflow source priority や builtin/workspace override semantics の設計変更が必要になる場合。
- session history を読まないと原因を特定できず、かつ transcript に private context が含まれる可能性がある場合。

## Validation

- prompt / workflow diff review。
- `git diff --check`。
- 必要に応じて `yoi ticket doctor`。
- 可能なら Intake の小さな再現シナリオで、曖昧な依頼に対して `TicketCreate` せず調査/draft に留まることを確認する。

## Related work

- `resources/prompts/role/intake.md`
- `resources/workflows/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-intake-workflow.md`
- `resources/profiles/intake.lua`
- `00001KTAZ2401`
- `00001KT0JPZS0`
- `00001KTRKZ14C`
- `00001KSKBPHRG`
