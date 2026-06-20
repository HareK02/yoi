<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:59:05Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により routing 対象として確認したが、`00001KVHR3WSW` は `00001KVHR3WS6` に `depends_on` している。list_changed handling は initial tools/list registration を前提にするため、`00001KVHR3WS6` が closed になるまで実装開始せず queued のまま保持する。

Next:
- `00001KVHR3WS6` が closed になった後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T10:07:00Z -->

## Decision

Routing decision: implementation_ready

Reason:
- User directive: 「blocker無いなら並列にやっちゃえよ」。現在 inprogress は 0 件であり、最後に残った queued Ticket の dependency blocker も解消済みのため開始する。
- 前回は `00001KVHR3WS6` initial tools/list registration が未完了だったため blocked/queued hold としたが、現在 `00001KVHR3WS6` は closed。
- Related resources/prompts operations Ticket `00001KVHR3WSN` も closed になっており、tools/resources/prompts list_changed をまとめて扱う context が揃っている。
- Ticket body は `notifications/tools/list_changed`, `notifications/resources/list_changed`, `notifications/prompts/list_changed` の handling、current run schema consistency、prompt-cache/history invariants、deterministic refresh/diagnostic behavior、unsafe refresh fallback を実装可能な粒度で定義している。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk domain は mcp / notifications / tool-schema / prompt-cache / refresh だが、Ticket は current run schema consistency、no hidden context mutation、bounded diagnostics を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WSW` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WSW)`: outgoing `depends_on 00001KVHR3WS6` is now closed。
- `TicketOrchestrationPlanQuery(00001KVHR3WSW)`: previous `blocked_by 00001KVHR3WS6` is resolved; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `b11f83c8`。
  - queued: this Ticket only。
  - inprogress: 0。
  - spawned child implementation Pods: 0。
  - no matching MCP list_changed branch/worktree。

IntentPacket:

Intent:
- Handle MCP list_changed notifications without silently staying stale forever and without mutating active-run model-visible tool schema or prompt/context history invariants unsafely。
- Implement a deterministic safe-boundary refresh / restart-required diagnostic / next-turn refresh policy that covers tools/resources/prompts list changes。

Binding decisions / invariants:
- Do not mutate current LLM context with hidden resource/prompt content。
- Do not unexpectedly mutate active run tool schema in a way that breaks request/history/prompt-cache invariants。
- list_changed notifications are signals; they should produce bounded state/diagnostic and deterministic refresh behavior at safe boundaries。
- Bounded diagnostics should identify server and list kind without leaking secrets。
- Preserve existing explicit Tool operations for tools/resources/prompts。
- Do not implement remote transports, sampling, or elicitation。

Requirements / acceptance criteria:
- Handle `notifications/tools/list_changed`, `notifications/resources/list_changed`, and `notifications/prompts/list_changed`。
- list_changed does not silently stale forever。
- Current run tool schema consistency is not broken。
- Refresh/diagnostic behavior is deterministic and documented。
- Prompt-context/history invariants are preserved。
- Tests cover tools/resources/prompts list_changed and unsafe refresh fallback。

Escalate if:
- Safe refresh requires changing Worker history/model schema lifecycle beyond this Ticket。
- Current provider/feature architecture cannot represent pending refresh diagnostics without broad redesign。
- A live schema mutation would be required to satisfy the feature expectation; stop instead and record restart/next-turn policy。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T10:07:07Z from: queued to: inprogress reason: orchestrator_acceptance_mcp_list_changed field: state -->

## State changed

User explicitly authorized implementation when no blocker exists. Ticket body/thread, relation metadata, closed MCP tool-registration dependency, closed resources/prompts context, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded list_changed/schema/history context were checked. Dependency `00001KVHR3WS6` is closed, no inprogress/capacity blocker exists, no matching worktree/branch exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T10:08:05Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `d31b8907 ticket: accept mcp list changed handling`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHR3WSW-mcp-list-changed` on branch `impl/00001KVHR3WSW-mcp-list-changed` at `d31b8907`。
- Spawned Coder Pod `yoi-coder-00001KVHR3WSW` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, hidden context injection, unsafe active-run schema mutation, sampling/elicitation scope creep, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T10:26:13Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHR3WSW`.

Implementation commit:
- `e33dee19 mcp: handle list changed notifications`

Changed areas reported:
- `crates/mcp/src/stdio.rs`:
  - Records bounded `notifications/tools/list_changed`, `notifications/resources/list_changed`, and `notifications/prompts/list_changed` state。
  - Ignores notification params entirely so notifications cannot inject prompt/resource content or hidden context。
  - Exposes snapshot/clear APIs for safe-boundary refresh decisions。
- `crates/pod/src/feature/mcp.rs`:
  - During startup tool discovery, refreshes `tools/list` once if `tools/list_changed` is observed before registration。
  - If list changes continue during refresh, emits a restart-required diagnostic and does not mutate active-run tool schema after registration。
  - During MCP tool/resource/prompt operations, appends bounded warnings to explicit tool output when list_changed is observed。
  - Preserves explicit operations for `tools/call`, `resources/list/read`, and `prompts/list/get`; no notification-driven content injection。
- Tests:
  - Added stdio notification state coverage for tools/resources/prompts。
  - Added provider/runtime tests for safe-boundary tool refresh, restart-required fallback, and resource/prompt warning behavior without leaking notification params。

Coder validation reported:
- `cargo test -p mcp list_changed -- --nocapture`: passed。
- `cargo test -p pod mcp::tests:: -- --nocapture`: passed。
- `cargo check --workspace`: passed。
- `cargo test -p mcp`: passed。
- `cargo test -p pod mcp::tests::`: passed。
- `cargo fmt --all -- --check`: passed。
- `git diff --check`: passed。
- `nix build .#yoi --no-link`: passed; dirty-tree warning expected before commit。

Known risks / deferrals:
- Live mutation of already-presented model-visible MCP tool schemas is intentionally not implemented。
- Continued `tools/list_changed` after one startup safe-boundary refresh produces bounded restart-required diagnostic。
- Resource/prompt list refresh remains explicit via existing MCP list/read/get tools on later turns; notifications alone do not inject or fetch content。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `e33dee19`。
- Diff from acceptance `d31b8907..HEAD` is one implementation commit touching 4 files, about 459 insertions / 9 deletions。
- `git diff --check d31b8907..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on current-run schema/history invariants, safe-boundary refresh policy, bounded diagnostics, no hidden prompt/resource injection, tools/resources/prompts notification coverage, and tests。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T10:26:52Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHR3WSW-r1` against implementation branch `impl/00001KVHR3WSW-mcp-list-changed`。
- Review target commit: `e33dee19 mcp: handle list changed notifications`。
- Review baseline: `d31b8907`。
- Reviewer task focuses on current-run schema/history invariants, safe-boundary refresh policy, bounded diagnostics, no hidden prompt/resource injection, notification param handling, tools/resources/prompts coverage, no sampling/elicitation/remote scope creep, tests, and package validation。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---
