<!-- event: create author: LocalTicketBackend at: 2026-06-08T06:12:35Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T06:15:37Z -->

## Decision

## Scope refinement: avoid idle starvation, not constant iteration

The phrase "continuously iterate" should not mean that the Orchestrator constantly re-kicks itself regardless of current work. The desired behavior is narrower:

- Do not leave newly queued work unnoticed while the Orchestrator is otherwise idle.
- Do not leave planned queued work unstarted when there is no in-progress work and capacity/policy allows starting it.
- Do not re-kick just because queued work exists if the Orchestrator is already waiting for an active coder/reviewer/preflight/merge step.

Refined planning model:

- `new_queued`: Tickets with `workflow_state = queued` that have not yet been incorporated into the OrchestrationPlan.
- `planned_queued`: queued Tickets that the Orchestrator has considered and placed into an explicit plan/order/waiting set, but has not yet accepted as `inprogress`.
- `inprogress`: Tickets accepted by the Orchestrator and currently awaiting worktree/coder/reviewer/preflight/merge/cleanup progress.

Desired scheduling/re-kick semantics:

1. If there is `new_queued` work and the Orchestrator is idle/not occupied by an active in-progress operation, it should be kicked or shown a bounded work list so it can incorporate the new Tickets into the plan.
2. If there is no `inprogress` work and there is `planned_queued` work that is not blocked/capacity-limited, the Orchestrator should be kicked to start/accept the next planned Ticket rather than waiting indefinitely for user instruction.
3. If there is active `inprogress` work whose next expected event is coder/reviewer/preflight/merge completion, do not re-kick merely because queued/planned work exists. The Orchestrator should wait for the active work or an explicit user action/capacity decision.
4. If planned queued work is blocked or waiting for capacity, the plan should record that reason so the panel/user can see why nothing starts.

This should be framed as starvation prevention and explicit work-set planning, not a background scheduler loop.

---

<!-- event: decision author: hare at: 2026-06-08T06:27:33Z -->

## Decision

## Dependency ordering: planning lane before re-kick planning layer

`replace-intake-state-with-planning` owns the behavior for cases where work cannot be planned/implemented yet because requirements or preflight are missing: return the Ticket to the planning lane with a visible reason.

This Ticket assumes that behavior exists and builds the `new_queued` / `planned_queued` / `inprogress` re-kick model on top of the clarified workflow states. Implementation order should be: settle `replace-intake-state-with-planning` first, then implement the OrchestrationPlan/re-kick layer.

Therefore, the OrchestrationPlan/re-kick layer should not introduce its own separate `preflight_needed` bucket or cleanup logic.

---

<!-- event: decision author: intake at: 2026-06-09T11:35:09Z -->

## Decision

## Intake refinement: ready scope for Orchestrator routing

既存 Ticket の本文と thread を確認し、関連 work と現在の Ticket lifecycle vocabulary に合わせて、残りの実装対象を以下に整理する。

### Scope after related work

- この Ticket は、Orchestrator が `queued` work を見落とさず、かつ active work を待っている最中に無駄な再 kick を繰り返さないための **active work set discovery / re-kick policy** を実装対象にする。
- OrchestrationPlan record/tool surface は既に別 Ticket で実装済みの前提として扱い、この Ticket で plan store 自体を再実装しない。
- 既存本文・thread の `workflow_state` という語は現在 schema の authoritative field ではない。実装では Ticket frontmatter の `state` を authority とし、既存記録中の `workflow_state = queued` などは historical wording / conceptual alias として読む。

### Binding decisions / invariants

- `new_queued` / `planned_queued` / `inprogress` は新しい core Ticket state として追加しない。必要な work-set classification は、現在の Ticket `state`、OrchestrationPlan records、role/session claim、visible Pod/worktree state から導出する。
- `queued` は Orchestrator が routing/start-if-unblocked を検討できる状態であり、implementation side effects は必ず `queued -> inprogress` が記録された後に限る。
- Panel や local lifecycle hook が Orchestrator に attention / kick を与えることはできるが、unattended scheduler loop や常時 polling にはしない。
- Orchestrator が active coder/reviewer/preflight/merge/cleanup 等の完了待ちである間は、queued/planned work が存在するだけでは再 kick しない。
- `planned_queued` work を開始しない理由が capacity / dependency / conflict / dirty workspace 等で説明できる場合は、bounded reason を OrchestrationPlan record または既存の適切な durable artifact に残し、Panel/user が理解できるようにする。
- duplicate start を避けるため、Ticket state、role/session claims、Plan records、visible Pod/worktree state を確認してから attention/kick/acceptance を行う。

### Implementation latitude

- どの runtime/panel boundary で idle Orchestrator を検出して kick するか、どの view-model/helper に work-set derivation を置くか、bounded payload の具体形は実装側で選んでよい。
- 既存 OrchestrationPlan types で waiting/capacity reason が足りない場合、最小の typed extension は検討してよい。ただし core Ticket lifecycle state の増殖や旧 `workflow_state` 復活は避ける。
- Panel 表示は state-first / heuristic-free の原則を維持する。title text、labels、thread event の有無、Pod 名 substrings を lifecycle authority にしない。

### Acceptance criteria

- Orchestrator が idle/not occupied のとき、new queued work を検出して plan incorporation または bounded work-list attention に進める。
- Inprogress work が存在せず、planned queued work が unblocked かつ capacity/policy 上開始可能なとき、Orchestrator が次の acceptance/routing を行える。
- Active inprogress operation の完了待ち中は、queued/planned work の存在だけで再 kick しない。
- 開始しない planned queued work には、ユーザーが確認できる bounded waiting/blocking reason が残る。
- 既存の human gate、`queued -> inprogress` acceptance step、dirty-workspace/dependency/conflict/capacity checks を迂回しない。
- duplicate Orchestrator/coder/reviewer/worktree start を起こさない。

### Validation

- `nix build .#yoi` を通す。
- Ticket / panel / orchestrator routing 周辺の既存テストまたは追加テストで、少なくとも idle queued detection、active-work wait suppression、waiting reason recording、duplicate-start prevention の主要分岐を確認する。
- 実装報告では、どの authority を work-set classification に使ったか、implementation side effects が `queued -> inprogress` 後に限定されていることを明示する。

### Readiness

- readiness: implementation_ready
- risk_flags: [orchestration-policy, panel-lifecycle, persistence, role-session, authority-boundary, duplicate-start]
- blocking open questions: none

この refinement により、Orchestrator はこの Ticket を implementation candidate として routing できる。

---

<!-- event: intake_summary author: intake at: 2026-06-09T11:35:29Z -->

## Intake summary

Ticket 00001KTJXS31R は implementation_ready。残り範囲は既に実装済みの OrchestrationPlan record/tool surface を前提にした active work set discovery / Orchestrator re-kick policy。`new_queued` / `planned_queued` / `inprogress` は新しい core Ticket state ではなく、現在の Ticket `state`、OrchestrationPlan records、role/session claims、visible Pod/worktree state から導出する分類として扱う。Panel/lifecycle hook は idle Orchestrator に bounded attention を与えるが、unattended scheduler loop や常時 polling にはしない。`queued -> inprogress` acceptance 前の implementation side effects、blind spawn、duplicate start は禁止。active coder/reviewer/preflight/merge/cleanup 待ち中は queued/planned work の存在だけで re-kick しない。risk_flags: [orchestration-policy, panel-lifecycle, persistence, role-session, authority-boundary, duplicate-start]。blocking open questions はない。

---

<!-- event: state_changed author: intake at: 2026-06-09T11:35:29Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、既存の plan store 実装との差分、current `state` vocabulary、binding invariants、implementation latitude、validation focus が整理され、Orchestrator が routing できる状態になった。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-12T14:49:40Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-12T14:56:17Z -->

## Decision

Routing decision: leave queued for now (`waiting_capacity_note`)

Reason:
- This queue review already accepted and delegated three in-progress branches: `00001KTVJFT6F` (Panel focus model), `00001KTTW04W2` (Companion progress notification), and `00001KTVJGC0Y` (Ticket language guidance).
- This Ticket concerns Orchestrator idle queued re-kick / active work-set behavior and overlaps conceptually with the active Companion progress notification and Panel lifecycle work. Starting it now would raise duplicate-start / scheduler-boundary / integration-conflict risk rather than safe independent parallelism.
- Relation check: dependency relation target `00001KTG3MDFG` is closed, so this is not blocked by an unresolved relation; the current stop reason is capacity/conflict with active work.

Evidence checked:
- Ticket body/thread: active in-progress suppression, clean workspace gate, no queue-drain loop, active-work-set snapshot requirements, and escalation conditions.
- TicketRelationQuery: two relation records checked; `depends_on` target `00001KTG3MDFG` is closed.
- TicketOrchestrationPlanQuery: no prior records; added `waiting_capacity_note` `orch-plan-20260612-145604-1`.
- Workspace/Pod state: Orchestrator worktree clean; active sibling coder Pods are running for Panel focus, Companion progress notification, and Ticket language guidance.

Next action:
- Keep `queued` and reconsider after at least Panel focus / Companion progress notification are merged or blocked, so this Ticket can validate against the current Panel/notification behavior without duplicate scheduler semantics.

---
