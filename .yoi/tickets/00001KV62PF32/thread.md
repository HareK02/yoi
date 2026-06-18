<!-- event: create author: "yoi ticket" at: 2026-06-15T16:44:06Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-17T09:46:03Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-17T09:49:06Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- Related `00001KV5MRH6D` は done で、first-frame latency E2E の前提を修正する follow-up として scope が明確。
- 本 Ticket は Panel startup readiness metric の E2E correction に限定され、Panel ViewModel architecture / lifecycle semantics / complete live latency fix は non-goal として明確。
- 同時 queued の Plugin WASM runtime work とは source surface が異なるため並行開始可能。

Evidence checked:
- Ticket body/thread: first frame vs rows ready distinction、fixture row assertion、tests、acceptance criteria、validation を確認。
- Ticket relations: outgoing `related 00001KV5MRH6D` のみで blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`bcb8068e` 上。
- Visible Pods: implementation child Pod なし。

IntentPacket:

Intent:
- Panel startup latency E2E の primary readiness を `panel_ready` / first frame から、fixture の具体的 Ticket/Pod/Orchestrator row が取得・描画された `panel_rows_ready` 相当に修正する。

Binding decisions / invariants:
- `panel_ready` は first visible frame として扱い、一覧 data-ready と混同しない。
- Primary startup latency assertion/report は fixture row render readiness に置く。
- Weak condition (`rows_rendered.len() >= N`) だけで pass にしない。
- Expected Ticket id/title/state/row kind 等の実データ反映を確認する。
- Background reload が hold されている間、first frame は出ても rows-ready は出ないことを確認する。
- Existing Panel behavior / Ticket lifecycle / ViewModel architecture は必要以上に変えない。
- E2E pass と manual/live validation を混同しない。

Requirements / acceptance criteria:
- E2E が concrete fixture Ticket row render timing を rows-ready readiness として測る。
- first frame だけでは main startup assertion が通らない。
- Helper/event names/comments で first frame と rows ready の違いが分かる。
- Delayed reload delays rows-ready metric but not necessarily first-frame metric。
- Row readiness fails if expected fixture row is absent。
- Existing Panel mouse / row selection E2E regression なし。
- Report includes what before/after numbers measure: first frame, rows ready, full/background completion if measured。

Implementation latitude:
- Existing `panel_ready` event can remain as first-frame event; add new `panel_rows_ready` event/helper/observer as needed。
- Fixture data may use Ticket id/title/state; Pod/orchestration overlay rows optional unless useful。
- Timing budget may reuse or adjust prior threshold, but must not permit 7s regression as pass without explanation。
- If current harness cannot observe row metadata reliably, add e2e-test-only observer rather than relying on screen string heuristics alone。

Escalate if:
- Current E2E harness cannot distinguish first frame from data-backed rows without broad redesign。
- Stable timing threshold for rows-ready is too flaky。
- Fixture row readiness depends on live workspace/global state rather than isolated fixture。
- Fix would require Panel architecture/lifecycle semantics outside Ticket scope。

Validation:
- `cargo test -p yoi-e2e --features e2e panel` or relevant narrow scenario。
- `cargo test -p yoi-e2e --features e2e --no-run`。
- `cargo check -p yoi-e2e -p yoi -p tui` if touched。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` if runtime/package/dependency changes occur。

Critical risks / reviewer focus:
- row readiness is data-backed, not first frame。
- expected fixture row specificity。
- delayed reload ordering proves distinction。
- no regression to Panel mouse / row selection E2E。
- E2E isolation / no-provider / no-network。
- report does not overclaim live startup fix。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-17T09:49:19Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation/evidence plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、Panel readiness E2E work は同時開始する Plugin WASM runtime work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-17T09:50:53Z -->

## Implementation report

Implementation start failure / recovery note:

`queued -> inprogress` acceptance と worktree 作成は完了したが、Coder Pod spawn に失敗した。

Created worktree:
- `/home/hare/Projects/yoi/.worktree/00001KV62PF32-panel-rows-ready-e2e`
- branch: `impl/00001KV62PF32-panel-rows-ready-e2e`

Failure:
- `SpawnPod` failed before child Pod creation.
- Error: `failed to spawn /home/hare/Projects/yoi/target/debug/yoi (deleted) pod: No such file or directory (os error 2)`

Interpretation:
- Runtime binary path for this long-lived Orchestrator process points at a deleted executable. This is an operational/runtime issue, not a Ticket requirement issue.
- No Coder Pod was started and no implementation commit exists in the child worktree yet.

Recovery needed:
- Refresh/restart the Orchestrator runtime or provide a valid Pod runtime command so `SpawnPod` can launch children again.
- After recovery, reuse the existing child worktree/branch if clean and resume Coder spawn for this in-progress Ticket.

State handling:
- Ticket remains `inprogress` because acceptance and worktree side effects already happened; it is not silently returned to `queued`.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T12:05:59Z -->

## Implementation report

Implementation resumed:

Previous `SpawnPod` failure was recovered after Orchestrator runtime refresh. Reused existing clean child worktree and successfully spawned Coder Pod.

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KV62PF32-panel-rows-ready-e2e`
- branch: `impl/00001KV62PF32-panel-rows-ready-e2e`

Coder Pod:
- `yoi-coder-00001KV62PF32`

No duplicate `queued -> inprogress` transition was performed; this resumes the already accepted in-progress work.

---
