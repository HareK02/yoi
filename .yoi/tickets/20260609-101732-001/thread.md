<!-- event: create author: LocalTicketBackend at: 2026-06-09T10:17:32Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T10:24:23Z -->

## Intake summary

Ticket 20260609-101732-001 は implementation_ready。目的は Orchestrator / multi-agent workflow guidance に parallel capacity policy を追加し、独立した queued work が複数あり capacity が空いている場合に、安全確認後は 1 件ずつ待たず並列受理・起動を優先するよう明文化すること。非目標は automatic background scheduler、unqueued Ticket の開始、OrchestrationPlan store/runtime scheduling の変更。既存安全不変条件（queued -> inprogress 前の副作用禁止、blind spawn 禁止、worktree/write-scope 分離、relation blocker 尊重、reviewer read-only 原則）は維持する。関連 Ticket 20260608-061235-001 は broader starvation-prevention/planning work であり、本 Ticket は workflow guidance に限定した non-blocking follow-up として扱う。risk_flags: [orchestration-policy, workflow-guidance, parallelism, authority-boundary, scope-isolation]。未解決の blocking question はない。

---

<!-- event: state_changed author: intake at: 2026-06-09T10:24:23Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件・受け入れ条件・非目標・安全不変条件・関連 work を確認した。実装は prompt/workflow resource の focused update と必要な snapshot/test 更新に限定でき、Orchestrator が routing 可能な状態。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:31:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-09T10:35:08Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_capacity field: state -->

## State changed

Accepted queued implementation under the updated parallel-capacity policy. This is workflow guidance for starting independent queued work in parallel, matching the user's current routing preference. It is docs/workflow-oriented and expected conflicts are small/manageable.

---

<!-- event: decision author: orchestrator at: 2026-06-09T10:35:08Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- The user explicitly requested more parallelism when conflicts are absent or small.
- This Ticket updates Orchestrator/multi-agent workflow guidance to encode that same policy.
- It is workflow/docs-oriented and can run alongside active code changes with expected small conflict risk.

IntentPacket:

Intent:
- Update Orchestrator and multi-agent workflow guidance so independent queued work with available capacity is started in parallel after explicit safety checks, rather than waiting one Ticket at a time by default.

Binding decisions / invariants:
- Orchestrator is not an automatic background scheduler.
- Do not start unqueued Tickets.
- No implementation side effects before `queued -> inprogress` acceptance.
- No blind spawn from queue notifications alone.
- Respect unresolved `depends_on` / incoming blocker relations, `do_not_parallelize`, conflict records, workspace dirty state, and shared write-scope constraints.
- Use separate worktrees/branches/write scopes for parallel Coder Pods.
- Reviewer remains read-only unless explicitly scoped otherwise.
- If queued work is left idle while capacity appears available, record a bounded reason: dependency, conflict, capacity, missing planning decision, workspace dirty state, reviewer/coder bottleneck, or human gate.
- Distinguish active work waiting on coder/reviewer output from idle Orchestrator queue-review moments.

Validation:
- Focused workflow/prompt text validation or tests showing parallel start is preferred when safety checks pass.
- Validation that safety invariants remain explicit.
- `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `nix build .#yoi` if packaged resources/docs are touched.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T10:36:44Z -->

## Implementation report

Implementation routing started in parallel with the current active work set.

Worktree/branch:
- Worktree: `.worktree/orchestrator-parallel-queued-work`
- Branch: `orchestrator-parallel-queued-work`
- Base/routing commit: `5d9e227 ticket: route analytics batching and parallel guidance`

Spawned sibling Coder Pod:
- `coder-orchestrator-parallel-queued-work`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Parallel-routing rationale:
- This workflow/docs Ticket encodes the user's updated parallel-capacity preference.
- It is expected to have only small/managed conflicts with active work and no runtime scheduler changes.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T10:42:37Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-orchestrator-parallel-queued-work`
- Commit: `492fe06832c30a08a982f469080041a290b1c228 workflow: prefer parallel queued starts`
- Worktree status before review: clean branch `orchestrator-parallel-queued-work`
- Stopped after collecting output to reclaim delegated worktree scope.

Implementation summary:
- Updated Orchestrator routing guidance to prefer starting additional independent queued Tickets during explicit queue review/routing when capacity exists, instead of waiting for the first active Ticket by default.
- Added required checks before parallel start: Ticket body/thread/artifacts, Ticket relations, OrchestrationPlan, dirty state, visible Pods, worktrees/branches, conflict/dependency notes.
- Preserved and strengthened the `queued -> inprogress` gate before implementation side effects.
- Explicitly disallowed starting unqueued Tickets just to fill capacity.
- Added bounded idle/defer reasons such as dependency, conflict, capacity, dirty workspace, migration boundary, and human gate.
- Updated multi-agent workflow to require separate worktrees/branches/narrow write scopes for parallel Coder Pods and read-only Reviewer default.
- Restated standing merge/validate/close/cleanup policy after reviewer approval, with migration/runtime/human gates as stop conditions.

Changed files:
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/multi-agent-workflow.md`

Coder validation reported passed:
- Focused text validation for parallel preference.
- Focused text validation for safety checks / bounded defer reasons.
- Focused text validation for no scheduler / no unqueued start.
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

`cargo check --workspace` was not run because only workflow resource/docs changed.

---
