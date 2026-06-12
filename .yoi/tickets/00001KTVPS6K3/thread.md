<!-- event: create author: LocalTicketBackend at: 2026-06-11T16:03:28Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T16:03:33Z -->

## Intake summary

Ticket role launch の初回 user message を短縮し、情報所管を Instruction / Workflow / Submit / control-plane に分離する方針で合意済み。language と workspace_root/cwd は Instruction 側の環境情報、Git/worktree 操作対象として必要な path は Submit に重複記載可。workflow slug/profile selector は初回 user message に含めない。

---

<!-- event: state_changed author: intake at: 2026-06-11T16:03:33Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・境界判断が揃ったため、Orchestrator routing 可能。実装はユーザーが panel で queue した後に開始する。

---

<!-- event: decision author: orchestrator at: 2026-06-12T09:11:30Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Workspace Panel Queue notification was received, but this Orchestrator backend still reads the Ticket as `ready`, not `queued`.
- The root workspace has unsynced/uncommitted queue-related changes for this Ticket and is dirty, including `.yoi/tickets/00001KTVPS6K3/*` and `crates/tui/src/multi_pod.rs`.
- Active in-progress work `00001KTWPE3KQ` is currently fixing the Panel Queue durable handoff/sync path and is in reviewer handoff; accepting this new Ticket before that path is reviewed/merged would require manual sync/queue recovery and could mix queue-side effects with unrelated dirty root changes.

Evidence checked:
- TicketShow `00001KTVPS6K3`: Orchestrator backend state is `ready`.
- TicketRelationQuery: no relation blockers.
- TicketOrchestrationPlanQuery: no prior plan records before this routing note.
- Root/orchestrator git state: Orchestrator branch has local routing record changes; root workspace is dirty with this Ticket's `.yoi` files and `crates/tui/src/multi_pod.rs`.
- Visible Pods: active reviewer `yoi-reviewer-panel-queue-sync` for `00001KTWPE3KQ`.

Next action:
- Leave this Ticket unaccepted for implementation in this Orchestrator pass.
- Re-route after the Panel Queue durable handoff work is resolved and the root/orchestration Ticket state is synchronized cleanly, or after a human explicitly instructs manual recovery for the queued root-side changes.

Escalate if:
- The queued root-side changes should be manually committed/synced despite the current dirty workspace and active Queue-handoff fix.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-12T12:29:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
