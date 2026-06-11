<!-- event: create author: "yoi ticket" at: 2026-06-11T05:15:14Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-11T07:40:32Z -->

## Plan

## Intake refinement

Readiness: implementation_ready

この Ticket は既存 Ticket `00001KTTB479X` の follow-up 境界として明確であり、Panel / workspace Orchestrator lifecycle で dedicated orchestration worktree を実際に作成・再利用し、その worktree を Orchestrator の runtime workspace / Ticket backend root として起動する具体的な work item になっている。

Orchestrator が routing できる判断材料:
- 目的、背景、要件、受け入れ条件、非目標が Ticket body に揃っている。
- `role_workspace_root` / `original_workspace_root` / `implementation_worktree_root` / `merge_target_workspace_root` の binding boundary が明記されている。
- implementation worktree は original workspace root 配下の `.worktree/<ticket-or-task>` に置き、orchestration worktree の子にしないことが invariant として明確。
- dirty / inconsistent / conflicting orchestration worktree は診断して止め、破壊的 cleanup や上書きをしないことが明確。
- 既存 worktree reuse、root population、`SpawnConfig.workspace_root`、prompt root context、Panel diagnostics について testable な acceptance criteria がある。

Risk flags / reviewer focus:
- `git-worktree`
- `ticket-backend`
- `workspace-state`
- `authority-boundary`
- `persistence`
- `panel-ux`

Open questions:
- blocking な未決定点はない。path / branch naming の具体実装、reuse validation helper の分割、UI diagnostic の表示場所は Ticket 内の制約を満たす範囲で implementation latitude として扱える。

Escalation conditions:
- Ticket backend を Git 外 store に移す必要が出た場合。
- Orchestrator push automation、破壊的 cleanup、main workspace publication policy の全面設計が必要になった場合。
- live conflicting Pod / inconsistent worktree の安全な診断だけでは足りず、人間判断が必要になった場合。

---

<!-- event: intake_summary author: intake at: 2026-06-11T07:40:39Z -->

## Intake summary

既存 Ticket を確認し、00001KTTB479X の closed follow-up として dedicated orchestration worktree を Panel/role launcher から作成・再利用し、Orchestrator の workspace root / Ticket backend root として起動する concrete work item と判断した。blocking な未決定点はなく、git-worktree / ticket-backend / workspace-state / authority-boundary / persistence / panel-ux を reviewer focus として残したうえで implementation_ready。実装は破壊的 cleanup、push automation、Git 外 Ticket store 移行、main workspace publication policy の全面設計を含めない。

---

<!-- event: state_changed author: intake at: 2026-06-11T07:40:39Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件・境界・受け入れ条件・risk focus が routing 可能な粒度まで確認されたため ready に遷移する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-11T07:41:37Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-11T07:42:27Z from: queued to: inprogress reason: orchestrator_queue_acceptance field: state -->

## State changed

Queue acceptance: relation query has no blockers, orchestration-plan query has no records, workspace has only this Ticket's panel queue update, and the Ticket is implementation_ready with concrete acceptance criteria for Panel Orchestrator lifecycle root separation. Standing merge-completion authorization is in effect after reviewer approval and safe-workspace checks. Implementation side effects may proceed after this recorded acceptance.

---

<!-- event: plan author: orchestrator at: 2026-06-11T07:42:27Z -->

## Plan

Routing decision: implementation_ready

Reason:
- Ticket is `queued`, user/panel authorized routing, relation query has no blockers, and orchestration-plan query has no existing records.
- Workspace dirty state before acceptance is limited to this Ticket's queue update.
- The Ticket has concrete requirements and acceptance criteria for Panel Orchestrator lifecycle dedicated orchestration worktree creation/reuse, root population, safe diagnostics, and tests.

Evidence checked:
- Ticket body/thread/artifacts.
- `TicketRelationQuery`: no blockers.
- `TicketOrchestrationPlanQuery`: no records before this routing pass.
- Workspace/worktree/branch state: main worktree only before acceptance.
- Visible Pods: no active implementation child Pods; restorable Intake peers only.

IntentPacket:
- Intent: make Panel Orchestrator launch create/reuse a dedicated orchestration worktree under the original workspace root and launch Orchestrator with that worktree as runtime workspace/Ticket backend root while preserving original/target roots for implementation worktree creation and merge-completion.
- Binding decisions / invariants: filesystem Ticket backend remains; no Git-external Ticket store; main workspace drafts are not implicit Orchestrator queue; implementation worktrees stay under original workspace `.worktree/<ticket-or-task>` as siblings of orchestration worktree; do not destructively cleanup dirty/unknown orchestration worktrees; no push automation; `SpawnConfig.workspace_root` for Orchestrator must be orchestration worktree.
- Requirements / acceptance criteria: stable collision-safe path/branch naming; safe create/reuse/fail diagnostics; `TicketRoleLaunchContext` roots population; prompt root context remains visible; Panel diagnostics make Orchestrator/Ticket backend root visible; tests for naming/reuse/unsafe diagnostics/root population/spawn config/prompt.
- Implementation latitude: exact helper placement and UI diagnostic wording are flexible if safety and root semantics are preserved.
- Escalate if implementation requires changing Ticket backend authority, adding destructive cleanup, or introducing push automation.
- Validation: focused panel/client tests, `cargo fmt --check`, `git diff --check`, `target/debug/yoi ticket doctor`, and `nix build .#yoi`.

---
