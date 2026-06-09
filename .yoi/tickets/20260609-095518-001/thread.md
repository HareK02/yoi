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

<!-- event: state_changed author: orchestrator at: 2026-06-09T11:30:25Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_capacity field: state -->

## State changed

Accepted queued implementation after TicketList and Panel display work landed and workspace is clean. This Ticket removes obsolete action/attention fields from current Ticket schema/tool/API/panel behavior and can proceed in parallel with Profile feature-flag work because expected conflicts are small/manageable.

---

<!-- event: decision author: orchestrator at: 2026-06-09T11:30:25Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- The earlier blockers are resolved: ToolExecutionContext, TicketList output slimming, and Panel composer/display work have landed.
- This Ticket is a bounded Ticket schema/API/panel cleanup to remove obsolete `action_required` and `attention_required` fields.
- It can run in parallel with Profile feature flags work because expected overlap is small/manageable.

IntentPacket:

Intent:
- Remove `action_required` and `attention_required` from current Ticket schema/tool/API/panel behavior, replacing any remaining current behavior with existing state/relation/orchestration-plan concepts where appropriate.

Binding decisions / invariants:
- New/current Ticket frontmatter must not include `action_required` or `attention_required`.
- Current parser/schema/tool/CLI/panel surfaces must not expose those fields as current metadata.
- Existing local Ticket records must be migrated to remove the fields.
- Do not replace them with another generic vague field.
- Use canonical `state`, Ticket relations, OrchestrationPlan waiting/blocker notes, and thread events for concrete state/context where appropriate.
- Historical thread/resolution prose may remain audit history unless it is a current fixture or maintained guidance.
- Do not change Ticket identity, lifecycle state model, relation metadata semantics, or Objective semantics.
- Do not implement a broad panel UX redesign here.

Reviewer focus:
- Verify all current frontmatter records are migrated.
- Verify TicketCreate/List/Show/tool schemas and Panel view models no longer carry action/attention fields.
- Verify maintained guidance no longer treats these fields as current authority.
- Verify doctor catches obsolete fields if appropriate.

Validation:
- Focused Ticket schema/parser/writer/tool tests.
- Focused yoi CLI/list/show tests and panel/view model tests if touched.
- `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `cargo check --workspace`, `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T11:31:52Z -->

## Implementation report

Implementation routing started in parallel with the Profile feature-flag work.

Worktree/branch:
- Worktree: `.worktree/remove-action-attention-ticket-fields`
- Branch: `remove-action-attention-ticket-fields`
- Base/routing commit: `41133e0 ticket: route feature flags and attention cleanup`

Spawned sibling Coder Pod:
- `coder-remove-action-attention-ticket-fields`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Parallel-routing rationale:
- ToolExecutionContext, TicketList, and Panel work have landed.
- This Ticket is primarily Ticket schema/tool/API/panel metadata cleanup.
- It is expected to have only small/manageable overlap with Profile feature-flag work.

---

<!-- event: implementation_report author: coder at: 2026-06-09T12:04:53Z -->

## Implementation report

Implemented removal of `action_required` and `attention_required` from current Ticket schema/tool/API/panel behavior.

Summary:
- Removed both fields from `NewTicket`, parsed/public metadata structs, list/show JSON output, TicketCreate params/schema, CLI list hints, and workspace panel row state.
- Removed Panel ready-ticket blocker and safe-close guards that depended on those fields; relation blockers continue to block via existing relation metadata.
- Added doctor obsolete-frontmatter diagnostics for both fields and updated focused tests to assert new writes/schema/output omit them.
- Updated maintained Intake/Orchestrator workflow guidance and development routing classification text away from the obsolete fields.
- Checked local `.yoi/tickets/**/item.md` frontmatter: no `action_required` or `attention_required` entries were present, so no non-null current values required migration or replacement.

Validation:
- `cargo test -p ticket`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui ticket_close_action`
- `cargo test -p yoi ticket`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- frontmatter grep/awk check for `.yoi/tickets/*/item.md`
- `nix build .#yoi`

---
