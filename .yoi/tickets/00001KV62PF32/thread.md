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

<!-- event: implementation_report author: yoi-coder-00001KV62PF32 at: 2026-06-18T12:19:45Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV62PF32`:

Commit:
- `fffdfd27 test: assert panel rows-ready fixture data`

Changed files:
- `tests/e2e/src/lib.rs`
  - Added concrete `ExpectedPanelTicketRow` matcher.
  - Exposed fixture ready/planning Ticket ids/titles/states.
  - Added `wait_for_first_visible_frame`, `wait_for_fixture_ticket_rows_ready`, and delayed-row negative assertion helpers.
- `tests/e2e/tests/panel.rs`
  - Replaced main startup readiness assertion from `rows.len() >= 2` / “full ready” to concrete ready fixture Ticket row readiness.
  - Added matcher regression test proving absent/wrong fixture data is rejected.
  - Added delayed reload assertion that first frame can arrive while fixture rows are still not rendered.
- `crates/tui/src/multi_pod.rs`
  - Added comments clarifying `panel_ready` is first-visible-frame only, while `rows_rendered` snapshots are the data-backed readiness surface.

Behavior / evidence:
- Main startup readiness now distinguishes:
  - first frame: legacy `panel_ready`, can be loading/empty;
  - rows ready: concrete `rows_rendered` containing expected fixture Ticket id/title/state/kind;
  - full/background completion: still observable via background task events, but no longer the startup readiness proxy.
- Startup readiness assertion is no longer satisfied by first frame alone or by weak `rows.len() >= N` checks.
- Delayed reload test asserts first visible frame can arrive while expected fixture Ticket row remains absent for the delayed window.

Validation reported by coder:
- Passed: `cargo test -p yoi-e2e --features e2e --no-run`
- Passed: `cargo test -p yoi-e2e --features e2e panel -- --nocapture`
- Passed: `cargo check -p yoi-e2e -p yoi -p tui`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`
- Not run: `nix build .#yoi` — no runtime/package/dependency changes.

Measured values from focused post-commit reruns:
- `panel_fixture_ticket_row_ready_has_startup_budget`
  - first visible frame: `20.403589ms` / budget `1.5s`
  - fixture rows ready: `120.605891ms` / budget `5s`
- `panel_first_visible_render_arrives_before_background_reload`
  - first visible frame: `20.311085ms` / budget `1.5s`
  - reload held; test asserts ready fixture Ticket row remains absent for delayed window before exit.

Repository status:
- Child implementation worktree clean at `fffdfd27`.

Residual gaps:
- None known.

---

<!-- event: review author: yoi-reviewer-00001KV62PF32 at: 2026-06-18T12:24:00Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- 対象 worktree / branch / commit が期待通りで clean であることを確認。
  - branch: `impl/00001KV62PF32-panel-rows-ready-e2e`
  - HEAD: `fffdfd2721fed5171d4dd9780f893b9bb323ab8a`
  - base: `d32fb3bc3c82f1cb7e4a29d4fdf6e908ba495c7f`
- Diff scope は以下3ファイルに限定されていることを確認。
  - `crates/tui/src/multi_pod.rs`
  - `tests/e2e/src/lib.rs`
  - `tests/e2e/tests/panel.rs`

Acceptance / intent review:
- Main startup readiness は `panel_fixture_ticket_row_ready_has_startup_budget` で concrete fixture Ticket row readiness を待つ形になっている。
- `rows.len() >= N` ではなく、fixture Ticket row の kind / id / title / status-state を matcher で確認している。
- `panel_ready` / `wait_for_first_visible_frame` は first-visible-frame only としてコメント・helper 名で区別されている。
- `wait_for_fixture_ticket_rows_ready` が startup rows-ready signal として定義されている。
- `panel_fixture_ticket_row_matcher_rejects_absent_fixture_data` により absent/wrong fixture data を reject する挙動が確認されている。
- delayed reload test は first frame が先に出る一方、held reload 中は expected fixture Ticket row が描画されないことを確認している。
- 既存 Panel mouse / row selection E2E は残っており、action dispatch なし・mouse capture behavior の coverage は維持されている。
- fixture isolation / no-provider / no-network 前提は維持されている。

Validation performed by reviewer:
- Passed: `cargo fmt --check`
- Passed: `git diff --check d32fb3bc..HEAD`

Validation not run by reviewer:
- `cargo test` / `cargo check` は read-only review scope では build/artifact 書き込みを伴うため未実行。Coder reported validation を evidence として確認した。

Risk assessment:
- residual risk は低い。変更は test/harness focused で Ticket boundary に沿っている。
- timing test は環境依存性が残るが、readiness predicate は first frame/loading paint ではなく concrete fixture row rendering を測るよう修正されている。

Conclusion:
- Approved. No blocker remains.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T12:25:14Z -->

## Implementation report

Integration report:

Merged implementation branch into orchestration branch:
- branch: `impl/00001KV62PF32-panel-rows-ready-e2e`
- implementation commit: `fffdfd27 test: assert panel rows-ready fixture data`
- merge commit: `226eca7a merge: panel rows-ready e2e`

Reviewer result:
- `approve` recorded from `yoi-reviewer-00001KV62PF32`.

Validation in Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`:
- Passed: `cargo test -p yoi-e2e --features e2e --no-run`
- Passed: `cargo test -p yoi-e2e --features e2e panel -- --nocapture`
  - panel E2E: 6 passed
  - first visible frame examples: ~20.34ms / budget 1.5s
  - fixture rows ready example: ~120.54ms / budget 5s
- Passed: `cargo check -p yoi-e2e -p yoi -p tui`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`

Outcome:
- Startup readiness E2E now uses concrete fixture Ticket row render readiness instead of first visible/loading frame or weak row-count checks.
- `panel_ready` remains first-visible-frame only; `rows_rendered` fixture row matching is the data-backed readiness surface.
- Delayed reload coverage verifies first frame can arrive before rows-ready and expected fixture Ticket row remains absent while reload is held.
- Existing Panel mouse/row selection E2E remains covered.

Cleanup planned:
- Stop related coder/reviewer Pods.
- Remove only child implementation worktree/branch for this Ticket.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-18T12:25:14Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Reviewer approved, implementation branch merged into the orchestration branch, and E2E-focused validation passed in the Orchestrator worktree. Marking Ticket done in the orchestration branch.

---

<!-- event: review author: hare at: 2026-06-18T13:30:51Z status: request_changes -->

## Review: request changes

Request changes.

The current result still does not answer the user-facing latency problem. The problematic latency is the time from launching `yoi panel` / pressing Enter to seeing the actual workspace dashboard content. The current E2E measures a direct subprocess spawn to one concrete fixture Ticket row appearing in `rows_rendered`; it does not require the dashboard content to be complete from the user's perspective, and it does not reproduce or attribute the clearly long live-workspace delay.

Do not treat fixture first-frame or single-row readiness numbers as evidence that no improvement is needed. The acceptance criterion must be strengthened to a user-visible dashboard-content-ready point and paired with slow-source attribution/improvement for the live-like Panel startup path.


---
