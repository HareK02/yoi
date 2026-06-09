<!-- event: create author: LocalTicketBackend at: 2026-06-09T09:55:18Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-09T10:02:25Z -->

## Intake summary

既存 Ticket を精査し、重複ではなく実装可能な concrete work item と判断した。目的は `action_required` / `attention_required` を current Ticket schema、TicketCreate/TicketList/TicketShow などの tool/API output、Panel action 判定から削除し、routing を止める不足は `state: planning`・typed thread reason・relation metadata に寄せること。非目標は typed relation metadata / Objective / 新しい Panel 確認 UX / planning-return policy 全体の再設計。リスク/レビュー重点は ticket-schema、tool-api、panel-behavior、workflow/docs 更新、legacy/current record compatibility。実装は current output から両 field を消し、ready Ticket の Queue 判定を `attention_required` に依存させず、既存 docs/workflows/tests/examples の current 記述を更新し、historical audit mention は残してよい。検証は Ticket doctor、focused Ticket/Panel/tool output tests、cargo fmt --check、git diff --check、必要に応じて `nix build .#yoi` を含める。関連する既存作業として explicit workflow state 導入 `20260606-215403-001`、YAML frontmatter strict 化 `20260607-223233-001`、Panel close safety `20260608-032200-001`、Ticket state commit policy `20260607-220606-001` を確認済みで、いずれもこの Ticket の duplicate ではない。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-09T10:02:25Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake classified this Ticket as ready to queue: the purpose, schema/tool/panel/doc scope, non-goals, acceptance criteria, validation expectations, and migration/compatibility boundary are explicit enough for Orchestrator routing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:11:38Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T10:12:07Z -->

## Decision

Routing decision: waiting_for_active_tool_api_migration

This Ticket is queued and appears implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- `20260609-093050-001` (`Tool実行にToolExecutionContextを渡す`) is already active, with Coder Pod `coder-tool-execution-context` running in `.worktree/tool-execution-context`.
- This Ticket touches Ticket schema, Ticket tool API output/input, and Panel action behavior.
- Running it in parallel with a core Tool execution API migration would create avoidable conflict across tool/Ticket surfaces and make review less reliable.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- Complete and merge `20260609-093050-001` first.
- Re-read this Ticket after that lands, then accept from queued if still unblocked.

---

<!-- event: decision author: orchestrator at: 2026-06-09T10:19:34Z -->

## Decision

Routing decision update: still_waiting_due_surface_overlap

The user asked to prefer parallel work when conflicts are absent or expected to be small. I re-evaluated this Ticket under that policy and am still leaving it queued.

Reason:
- This Ticket removes `action_required` / `attention_required` from Ticket schema/tool API/Panel behavior.
- `20260609-085212-001` now runs in parallel and also changes TicketList/tool output.
- `20260609-084725-001` now runs in parallel and changes Panel behavior/key/display.
- Running this cleanup at the same time would overlap both active Ticket tool output and Panel behavior surfaces; this is more than a small mechanical conflict.

State decision:
- Leave this Ticket `queued`.
- Re-read and route it after either TicketList output slimming or Panel display work lands, depending on which surface remains active.

---
