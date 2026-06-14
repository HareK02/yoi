<!-- event: create author: intake at: 2026-06-13T10:54:34Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-13T10:54:41Z -->

## Intake summary

ユーザー承認済みの新規 Ticket。Panel の `ready` Ticket に対し、Queue 以外にユーザー指示付きで `planning` へ戻し、同じ Ticket の Intake を restore / launch して再詳細化できる操作を追加する。実装開始ではなく requirements sync / refinement 導線であり、typed Ticket backend 経由の `ready -> planning` 記録、stale state 再確認、指示の thread 永続化、Intake 起動結果の bounded 表示、Queue / Orchestrator / worktree / coder/reviewer side effect 不発生が受け入れ条件。

---

<!-- event: state_changed author: intake at: 2026-06-13T10:54:41Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement completed. ユーザーが draft を承認し、意図・受け入れ条件・binding invariants・implementation latitude・escalation conditions・validation が Orchestrator routing 可能な粒度で揃っている。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T18:41:14Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、ready Ticket を Panel からユーザー指示付きで `planning` に戻し Intake を再開する action の intent / requirements / acceptance criteria / invariants が具体化されている。
- `TicketRelationQuery` / `TicketOrchestrationPlanQuery` に blocker はない。
- Risk は panel-action / ticket-lifecycle / role-session / authority-boundary だが、対象は `ready -> planning` のみ、typed backend 経由、Queue/Orchestrator/worktree/coder side effect 不発生という binding invariants が明記済み。
- `00001KV0X254D` と同じ Panel 周辺に触れる可能性はあるが action dispatch と orchestration branch config は論理的に分離できる。merge conflict が出たらその Ticket を止めて報告する。

Evidence checked:
- Ticket body / thread / artifacts。
- relation records: なし。
- orchestration plan records: なし。
- workspace state: Orchestrator worktree clean、dedicated child worktree で実装予定。

IntentPacket:
- Panel の ready Ticket row から Queue とは別に refinement/intake 再開 action を実行できるようにし、ユーザー指示を Ticket thread に保存して typed `ready -> planning` を記録し、Intake restore/launch を試行する。

Binding decisions / invariants:
- この action は implementation routing ではなく requirements sync への戻し。
- `queued` / `inprogress` は範囲外。
- stale state は再読込して拒否する。
- Queue、`queued -> inprogress`、worktree 作成、coder/reviewer spawn は発生させない。

Validation:
- Focused Panel/action tests、Ticket lifecycle/role-session tests、`cargo test -p tui workspace_panel`、`cargo test -p ticket`、`cargo fmt --check`、`git diff --check`。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T18:41:25Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence、relation records、orchestration plan、workspace state を確認した。Blocking relation はなく、implementation-ready。実装 side effect の前に inprogress acceptance を記録する。

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T18:42:45Z -->

## Implementation report

Implementation handoff started.

- Worktree: `/home/hare/Projects/yoi/.worktree/panel-return-planning`
- Branch: `ticket-00001KV09X0XC-panel-return-planning`
- Coder Pod: `coder-00001KV09X0XC-return-planning`
- Scope: root workspace non-recursive read for launch validation only; child worktree read; writes limited to relevant TUI/Ticket/Client/Yoi/Pod role-session surfaces, target, and this Ticket record。

Next: Coder implementation_report / commit / validation evidence を確認後、Reviewer を起動する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T19:01:36Z -->

## Implementation report

Commit: `f74146c6` (`tui: return ready tickets to planning from panel`)

Files changed:
- `crates/tui/src/multi_pod.rs`

What changed:
- Added a separate Panel path for selected `ready` Ticket rows when the composer target is Ticket Intake and the user submits non-empty refinement instructions.
- The action rereads the Ticket through the typed local Ticket backend and records a typed `ready -> planning` `state_changed` event with the user's refinement instruction in the Ticket thread.
- The action does not invoke Queue, `queued -> inprogress`, worktree creation, or implementation-role spawning.
- After the state change it attempts the Intake continuation path:
  - launch and claim a new Intake Pod when no local Intake claim exists,
  - notify a live claimed Intake Pod,
  - open/restore a restorable claimed Intake Pod,
  - or leave a visible diagnostic for stale claims without launching a duplicate.
- Added focused tests for stale-state rejection, state/event recording, action dispatch selection, launch-after-mutation followup, and role-session claim recording for post-launch Intake Pods.

Validation:
- `cargo test -p tui planning_return`
- `cargo test -p tui intake_registry`
- `cargo test -p tui workspace_panel`
- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`

Residual risks:
- No real terminal/PTTY Panel E2E was run; coverage is focused unit/action-path validation.
- The existing Panel composer model exposes this via Ticket Intake target + selected ready row + non-empty text, rather than adding a new visible button/menu surface.

---

<!-- event: review author: hare at: 2026-06-13T19:07:43Z status: approve -->

## Review: approve

Evidence reviewed:
- Inspected `git diff c4465a04..HEAD`; source changes are confined to `crates/tui/src/multi_pod.rs` plus Ticket record/report updates.
- Panel dispatch now has a separate `ReturnReadyTicketToPlanning` path for non-empty composer text when the composer target is Ticket Intake and the selected row action is `Queue`/ready.
- The mutation path reloads the Ticket from `LocalTicketBackend`, rejects non-`ready` current state, and records a typed `ready -> planning` `state_changed` event with author `workspace-panel`, reason `panel_return_to_planning`, and the user instruction in the thread body.
- The follow-up paths are Intake-only: launch when unclaimed, notify live claimed Intake, open/restorable claimed Intake, or diagnose stale claim without duplicate launch. I did not find calls to queue dispatch, `queued -> inprogress`, worktree creation, Orchestrator/Coder/Reviewer spawn in this path.
- Tests cover successful planning return, stale-state rejection, dispatch separation from Queue/generic Intake launch, state-before-launch follow-up, and launched-claim registry handling. Existing queue action coverage remains present.

Validation run:
- `cargo test -p tui planning_return` — pass (4 tests)
- `cargo test -p tui intake_registry` — pass (4 tests)
- `cargo test -p tui workspace_panel` — pass (12 tests)
- `cargo test -p ticket` — pass (68 tests + doctests)
- `cargo fmt --check` — pass
- `git diff --check c4465a04..HEAD` — pass

Residual notes:
- No real terminal/PTTY Panel E2E was run; this remains a unit/action-path review only.
- Discoverability depends on the existing composer target model: select a ready Ticket row, switch to Ticket Intake, type non-empty refinement instructions, then Enter. The implementation makes this visible in status/actionbar text; no separate button/menu was added.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-14T05:08:58Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KV09X0XC-panel-return-planning`
- Implementation commits: `f74146c6 tui: return ready tickets to planning from panel`, `cb565477 ticket: report panel planning return implementation`
- Review commit: `9d4abe50 ticket: approve panel planning return`
- Orchestrator merge commit: `7a6321d9 merge: panel return planning`

Reviewer result:
- `approve`。ready Ticket row + Ticket Intake target + non-empty instruction のみで typed `ready -> planning` を記録し、stale state を拒否し、Queue / worktree / implementation Pod side effects を発生させないことを確認済み。

Orchestrator validation after merge:
- `cargo test -p tui planning_return`: PASS
- `cargo test -p tui intake_registry`: PASS
- `cargo test -p tui workspace_panel`: PASS
- `cargo test -p ticket`: PASS
- `cargo fmt --check`: PASS
- `git diff --check`: PASS

Residual notes:
- Review note の通り、real terminal/PTTY Panel E2E は未実施。coverage は focused unit/action-path validation。
- Discoverability は既存 composer target model（Ticket Intake target + ready row selection + non-empty text）に依存し、独立ボタン/メニューは追加していない。

Next:
- Mark Ticket done and clean up child coder/reviewer Pods plus implementation worktree/branch.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T05:09:07Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Implementation branch was reviewed, approved, merged into the Orchestrator branch as `7a6321d9`, and validated in the Orchestrator worktree. Focused TUI planning-return/intake/workspace-panel tests, Ticket tests, formatting, and diff check passed. Ticket implementation work is done; closure remains separate.

---

<!-- event: state_changed author: hare at: 2026-06-14T14:00:13Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-14T14:00:13Z status: closed -->

## 完了

Closed after prior done-state completion.


---
