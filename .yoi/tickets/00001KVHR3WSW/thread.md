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

<!-- event: review author: yoi-reviewer-00001KVHR3WSW-r1 at: 2026-06-20T10:31:26Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket `00001KVHR3WSW` item/thread、Orchestrator IntentPacket、binding invariants。
- Diff: `d31b8907..e33dee19`。
- 主な対象:
  - `crates/mcp/src/stdio.rs`
  - `crates/mcp/tests/stdio_lifecycle.rs`
  - `crates/mcp/tests/fixtures/mock_server.rs`
  - `crates/pod/src/feature/mcp.rs`
- Hidden context/history append、post-registration live ToolRegistry mutation、notification param usage/leakage、sampling/elicitation/remote transport scope creep の有無を確認。

Blocking issues: none。

Approval evidence:
- `crates/mcp/src/stdio.rs` は `notifications/tools/list_changed`, `notifications/resources/list_changed`, `notifications/prompts/list_changed` を bounded enum state としてのみ記録している。
- Notification `params` は incoming message shape として deserialize されるが、保存・render・behavior には使われない。
- Startup discovery は model-visible tool registration 前に `tools/list_changed` を検出した場合、`tools/list` refresh を最大 1 回だけ行う。
- Refresh 後も changes が続く場合は restart-required diagnostic を出し、post-registration ToolRegistry/schema mutation は行わない。
- Runtime MCP tool/resource/prompt operations は list-change notification を ordinary Tool output の bounded warning として明示的に返す。Hidden resource/prompt content injection はない。
- Sampling / elicitation / remote transport scope creep はない。
- Tests は kind-only notification state、safe-boundary tool refresh、continued-change restart diagnostic、resources/prompts warnings、malicious notification params が漏れないことを cover している。

Non-blocking follow-ups:
- Runtime diagnostic wording に “restart the Pod or start a new run to rediscover tools” とある。MCP discovery が Pod-startup scoped であるなら、将来 “restart/reinitialize the Pod” のみに絞るとより明確。
- Resources/prompts `list_changed` notifications observed during startup discovery は、explicit resource/prompt tool operations 中でない限り別途 surface されない。ただし resources/prompts lists は startup で cache されないため、この Ticket では blocking ではない。将来 persistent resource cache を持つなら明示 diagnostics が必要。

Reviewer validation:
- `cargo fmt --all --check`: passed。
- `git diff --check d31b8907..HEAD`: passed。
- `cargo test -p mcp list_changed -- --nocapture`: passed。
- `cargo test -p pod mcp::tests:: -- --nocapture`: passed, 16 tests。
- `cargo test -p mcp`: passed, 13 tests。
- `cargo check --workspace`: passed。
- `nix build .#yoi --no-link`: not run by Reviewer。

Worktree status at review end: source tree clean; ignored `target/` only。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T10:32:28Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `ae5f3e42 merge: mcp list changed handling`
- Implementation commit included:
  - `e33dee19 mcp: handle list changed notifications`

Reviewer outcome:
- r1 approved with no blocking issues。
- Reviewer confirmed bounded kind-only list_changed state, ignored notification params, safe-boundary startup refresh, restart-required fallback, no post-registration live schema mutation, no hidden resource/prompt injection, no sampling/elicitation/remote scope creep, and focused tests。

Orchestrator validation after merge passed:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp list_changed -- --nocapture`
- `cargo test -p pod mcp::tests:: -- --nocapture`
- `cargo test -p mcp`
- `cargo check --workspace`
- `nix build .#yoi --no-link`
- `nix path-info -S .#yoi`: `113428296`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-ddp5Ei.log`

Final state:
- Orchestrator worktree clean at `ae5f3e42` after validation。
- Implementation worktree remains available for cleanup after Ticket completion records are committed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T10:32:35Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `ae5f3e42`, review approved, and final Orchestrator validation passed: `cargo fmt --all --check`, `git diff --check HEAD^1..HEAD`, focused `mcp` and `pod mcp::tests::` tests, `cargo check --workspace`, and `nix build .#yoi --no-link`.

---

<!-- event: state_changed author: hare at: 2026-06-20T10:32:59Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-20T10:32:59Z status: closed -->

## 完了

## Resolution

`00001KVHR3WSW` を完了しました。

実装内容:
- MCP `notifications/tools/list_changed`, `notifications/resources/list_changed`, `notifications/prompts/list_changed` を bounded kind-only state として記録します。
- Notification params は保存・render・behavior に使わず、hidden resource/prompt context injection を防止します。
- Safe-boundary refresh 用の snapshot/clear API を追加しました。
- Startup tool discovery では、registration 前に `tools/list_changed` が観測された場合のみ `tools/list` を最大 1 回 refresh します。
- Refresh 後も変更が続く場合は bounded restart-required diagnostic を出し、active-run model-visible tool schema を post-registration mutation しません。
- MCP tool/resource/prompt operations 中に list_changed が観測された場合、ordinary Tool output に bounded warning を明示的に返します。
- Resource/prompt notifications は content fetch/injection を行わず、explicit list/read/get tools でのみ扱います。
- Sampling / elicitation / remote transport は実装していません。

主な commit:
- `e33dee19 mcp: handle list changed notifications`
- `ae5f3e42 merge: mcp list changed handling`

Review:
- r1 は `approve`。
- Reviewer は current-run schema/history invariants、safe-boundary refresh、restart-required fallback、notification params の非使用、no hidden injection、no sampling/elicitation/remote scope creep を確認しました。

最終 validation:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp list_changed -- --nocapture`
- `cargo test -p pod mcp::tests:: -- --nocapture`
- `cargo test -p mcp`
- `cargo check --workspace`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113428296`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-ddp5Ei.log`

---
