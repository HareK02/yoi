## Resolution

`00001KVJDJD02` を完了しました。

実装内容:
- `resources/prompts/role/intake.md` に official `TicketCreate` 前の minimum investigation gate を追加しました。
- Intake が user claims / confirmed facts / unverified hypotheses / undecided points を区別するように model-facing guidance を補強しました。
- User agreement before official Ticket creation を維持・明確化しました。
- Intake non-scheduler boundary を補強しました。
  - coder/reviewer/read-only helper Pod spawn なし。
  - worktree作成なし。
  - implementation/review routing、merge、close なし。
- `resources/workflows/ticket-intake-workflow.md` を concrete reusable Intake procedure に拡張しました。
- `.yoi/workflow/ticket-intake-workflow.md` を bundled workflow と整合させつつ、dogfooding/workspace-specific details を維持しました。
- Investigation が必要な場合、`requirements_sync_needed` / `spike_needed` / `blocked` の draft stop behavior を明示しました。
- `Action required` / `Attention required` の stale wording を touched templates から削除し、current Ticket-operation vocabulary に置換しました。

主な commit:
- `1143ae1c workflow: add intake investigation gate`
- `f62ed4db merge: intake investigation gate`

Review:
- r1 は `approve`。
- Reviewer は Intake non-scheduler boundary、user agreement、Ticket 化前 investigation gate、draft stop behavior、claims/facts/hypotheses/open questions separation、bundled/workspace workflow consistency、stale vocabulary removal を確認しました。

最終 validation:
- `git diff --check HEAD^1..HEAD`
- stale vocabulary grep: `Action required` / `Attention required` no matches in touched files。
- investigation vocabulary grep: expected terms present。
- `TicketDoctor`: 0 errors。

Known unrelated note:
- `TicketDoctor` は既存 Ticket の warning 4 件を返しましたが、この Ticket の変更とは無関係です。