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

<!-- event: decision author: orchestrator at: 2026-06-12T15:46:19Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 前回の waiting reason は、Panel focus / Companion progress notification / Ticket language guidance の active in-progress work と重なる capacity/conflict だった。現在それらは merge/close/cleanup 済みで、visible child Pod は 0、Orchestrator worktree は clean。
- Relation blocker はない。`depends_on` target `00001KTG3MDFG` は closed 済みで、incoming `related` は non-blocking context である。
- Ticket body/thread は、session-lifetime work set discovery / re-kick policy、active_inprogress suppression、duplicate-start prevention、no scheduler/no polling/no blind spawn、`queued -> inprogress` acceptance gate を binding decisions として明確にしている。
- risk flags は orchestration-policy / panel-lifecycle / persistence / role-session / authority-boundary / duplicate-start だが、bounded context check の結果、具体的な未決定 design/API/authority 判断は残っていない。実装方式は Panel/lifecycle/runtime boundary と local session work-set representation の local tactic に閉じている。
- OrchestrationPlan に accepted plan `orch-plan-20260612-154541-2` を記録済み。

Evidence checked:
- Ticket body / thread / artifacts: requirements, acceptance criteria, binding decisions, previous waiting_capacity_note, and accepted plan を確認。
- TicketRelationQuery: `depends_on` target `00001KTG3MDFG` closed、incoming `related` non-blocking。
- Related Ticket: `00001KTG3MDFG` closed。OrchestrationPlan record/tool surface は実装済みで、この Ticket では再実装しない前提を確認。
- Recent merged context: Panel focus model、Companion weak progress notify、Ticket language guidance は closed/merged/cleaned up 済み。
- Code map: `crates/tui/src/workspace_panel.rs`, `crates/tui/src/multi_pod.rs`, `crates/tui/src/role_session_registry.rs`, Ticket state/action surfaces, Pod visibility/state surfaces, recent `companion_progress` weak notify path を確認。
- Workspace/Pod state: Orchestrator branch `orchestration/yoi-orchestrator` is clean; visible child Pods 0。

IntentPacket:

Intent:
- Orchestrator が idle で actionable queued work を見落とすことを防ぐ bounded starvation-prevention layer を実装する。
- `active_inprogress` が導出されている間は queued/planned work があるだけでは re-kick しない。
- re-kick は inspection / work-set incorporation / next planned acceptance を促す attention であり、scheduler や blind implementation start ではない。

Binding decisions / invariants:
- `new_queued` / `planned_queued` / `active_inprogress` / `actionable_inprogress` は core Ticket state に追加しない。Ticket `state`、session-lifetime work set、role/session claims、visible Pod/worktree state、OrchestrationPlan records から導出する分類に留める。
- Work set は session-lifetime runtime state であり、Ticket ごとの durable artifact log として積まない。durable に残すべき判断だけ Ticket comment / state transition / OrchestrationPlan record に残す。
- 常時 polling / unattended scheduler loop / queue drain loop を作らない。
- `queued -> inprogress` acceptance なしに coder/reviewer Pod spawn、worktree 作成、implementation side effect を行わない。
- active coder/reviewer/planning-sync/merge/cleanup 等の完了待ち中は queued/planned work の存在だけで re-kick しない。
- duplicate Orchestrator/coder/reviewer/worktree start を防ぐ。
- Panel は authority/backend/scheduler にならず、bounded attention surface に留まる。

Requirements / acceptance criteria:
- Orchestrator Pod が `Idle` で `new_queued` work があるとき、bounded work-list attention または session work set incorporation に進める。
- `active_inprogress` がなく、planned queued work が unblocked/capacity-allowed のとき、next acceptance/routing に進める。
- `active_inprogress` が導出される間は queued/planned work の存在だけで re-kick しない。
- 開始しない planned queued work には session work set 上でユーザーに提示できる bounded waiting/blocking reason を保持する。
- Ticket state、session work set、role/session claims、visible Pod/worktree state を使って duplicate start を避ける。
- session work set が失われても `queued` Ticket から安全に再検出・再 inspection できる。

Implementation latitude:
- Panel open / Orchestrator restore/spawn / explicit user action のどの boundary で idle detection と attention payload を組むかは実装側で選んでよい。
- Session work-set の具体的な runtime representation は既存 local role/session registry や Panel app state に合わせて選んでよい。
- Bounded attention payload の具体形は実装側で選んでよいが、full Ticket thread / unbounded Pod output / hidden context-only injection は避ける。
- OrchestrationPlan types の最小拡張は必要なら検討してよいが、core Ticket lifecycle state や旧 `workflow_state` は復活させない。

Escalate if:
- Durable scheduler state / persistent queue runner / polling loop が必要になる。
- `queued -> inprogress` acceptance gate を迂回しないと実装できない。
- Duplicate start prevention に新しい global lock/lease authority が必要になる。
- Panel が lifecycle authority / scheduler になる必要が出る。
- Role/session claims から active_inprogress を安全に導出できない。

Validation:
- idle Orchestrator + queued detection。
- active_inprogress 導出時の re-kick suppression。
- planned queued waiting reason retention。
- duplicate-start prevention。
- session work set loss から queued Ticket 再検出。
- relevant `cargo test` for `tui` / panel / role session / ticket routing paths。
- `cargo fmt --check`。
- `git diff --check`。
- `cargo run -p yoi -- ticket doctor` または同等。
- `nix build .#yoi`。

Current code map:
- `crates/tui/src/workspace_panel.rs`: Ticket rows/actions and state-first Panel view model。
- `crates/tui/src/multi_pod.rs`: Panel lifecycle, Companion/Orchestrator interactions, recent progress notice / header behavior。
- `crates/tui/src/role_session_registry.rs`: local role/session claims。
- `crates/pod-store`, `crates/pod-registry`, and protocol Pod status surfaces only if needed for visible Pod/worktree state derivation。
- Ticket relation / OrchestrationPlan tool surfaces are already implemented and should be reused/read, not reimplemented as plan store。

Critical risks / reviewer focus:
- No scheduler / queue drain loop / background polling。
- No blind worktree/Pod spawn before `queued -> inprogress` acceptance。
- active_inprogress suppression works and avoids re-kick while coder/reviewer/merge/cleanup is active。
- duplicate start prevention uses actual Ticket state + claims + visible Pods/worktrees, not heuristic title/labels。
- session work set is runtime/session scoped, not a new durable per-Ticket artifact log。
- Recent Companion weak progress notify and Panel focus behavior remain coherent.

Next action:
- `queued -> inprogress` を記録してから、branch `ticket/orchestrator-idle-queued-rekick` / worktree `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick` を作成し、sibling coder に narrow write scope で実装を委譲する。Reviewer は coder evidence 後に read-only で起動する。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T15:46:28Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing accepted for implementation. Ticket body/thread, dependency relation target, incoming non-blocking relation, previous waiting-capacity note, current Orchestrator workspace state, visible Pods, recent merged Panel/Companion/Ticket-language context, and relevant Panel/lifecycle/role-session code map were rechecked. No unresolved blocker or missing planning decision remains. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded, using accepted plan `orch-plan-20260612-154541-2`.

---

<!-- event: plan author: orchestrator at: 2026-06-12T15:47:00Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTJXS31R`
- Branch: `ticket/orchestrator-idle-queued-rekick`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick`
- Base: Orchestrator workspace HEAD `e72a4536` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Scope rule: implementation work happens only in this child worktree. Root/original workspace must not be read, written, validated, merged, cleaned, or used for git operations.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---

<!-- event: plan author: orchestrator at: 2026-06-12T15:47:50Z -->

## Plan

Coder delegated.

- Coder Pod: `yoi-coder-idle-queued-rekick`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick`
- Branch: `ticket/orchestrator-idle-queued-rekick`
- Scope: write `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick`; read child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Task: implement bounded idle-Orchestrator queued-work attention / session work-set rediscovery without scheduler/polling/queue-drain behavior, preserve `queued -> inprogress` acceptance gate, suppress re-kick while active_inprogress work exists, prevent duplicate starts, and add focused tests plus required validation.
- Constraint: coder was instructed not to use root/original workspace for decision evidence or git/validation/cleanup, and not to create generated memory/local/runtime/log/lock/secret-like `.yoi` paths.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T16:04:17Z -->

## Implementation report

Coder completed implementation.

Coder Pod:
- `yoi-coder-idle-queued-rekick`

Implementation branch/worktree:
- Branch: `ticket/orchestrator-idle-queued-rekick`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick`
- Commit: `d2fae81a tui: add idle queued orchestrator attention`
- Worktree status: clean

Changed surfaces:
- `crates/tui/src/multi_pod.rs`
- `resources/prompts/panel/orchestrator_idle_queue_notice.md`

Implementation summary:
- Adds session-scoped `OrchestratorWorkSet` derived at Panel reload boundary.
- Derives `active_inprogress`, new/actionable queued, planned queued, waiting reasons, and duplicate guards from Panel Ticket rows, local role-session claims, related Pods, blocked reasons, and live Orchestrator status.
- Sends bounded Orchestrator attention only when Orchestrator is live/reachable/idle and queued work is actionable.
- Suppresses attention when active_inprogress exists or duplicate-start guard indicates existing claim/Pod/worktree activity.
- Retains waiting reason / planned queued information in Panel header detail for session-visible explanation.
- Adds prompt resource `resources/prompts/panel/orchestrator_idle_queue_notice.md` for the attention payload, preserving the prompt-resource boundary.
- Does not add scheduler loop, polling loop, queue drain loop, core Ticket states, or persistent work-set artifact store.

Implementation note for reviewer attention:
- Coder report used the phrase `workflow_state` for `TicketPanelEntry.workflow_state`; this is existing Panel model naming over the typed Ticket state enum, not reintroduction of obsolete Ticket frontmatter `workflow_state`.

Orchestrator-verified evidence from child worktree:
- `git status --short --branch`: clean on `ticket/orchestrator-idle-queued-rekick`
- `cargo test -p tui queued_attention`: pass, 3 tests
- `cargo test -p tui planned_queued_prompts`: pass, 1 test
- `cargo test -p tui rediscovered_queued_work`: pass, 1 test
- `cargo test -p tui active_inprogress_suppresses`: pass, 1 test
- `cargo test -p tui idle_orchestrator_gets_bounded_attention`: pass, 1 test
- `cargo test -p tui workspace_panel`: pass, 12 tests
- `cargo check -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known broad-suite note from coder:
- `cargo test -p tui` still fails unrelated existing/neighboring expectation tests:
  - `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`
  - `spawn::tests::profile_choices_use_project_registry_default`
  - `spawn::tests::profile_choices_include_builtin_and_project_default_marker`
- Focused tests for this Ticket pass; reviewer should confirm broad failures are unrelated and not introduced by this Ticket.

Next:
- Spawn read-only sibling reviewer against recorded intent, binding decisions/invariants, acceptance criteria, commit `d2fae81a`, diff, validation evidence, and known broad-suite failures.

---
