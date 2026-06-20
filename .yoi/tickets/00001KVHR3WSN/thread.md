<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:58:57Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により routing 対象として確認したが、`00001KVHR3WSN` は `00001KVHR3WRY` に `depends_on` している。MCP resources/prompts operations は initialized stdio lifecycle を前提にするため、`00001KVHR3WRY` が closed になるまで実装開始せず queued のまま保持する。

Next:
- `00001KVHR3WRY` が closed になった後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T09:37:14Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- User directive: 「blocker無いなら並列にやっちゃえよ」。現在 inprogress は 0 件であり、依存 blocker も解消済みのため、この queued Ticket を開始する。
- 前回は `00001KVHR3WRY` stdio lifecycle が未完了だったため blocked/queued hold としたが、現在 `00001KVHR3WRY` は closed。
- Ticket body は resources/list, resources/read, prompts/list, prompts/get を explicit tool operations として exposeし、hidden context injection を禁止し、ordinary Tool result/history path・untrusted/bounded content handling・pagination/list bounds を明確にしている。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk domain は mcp / resources / prompts / prompt-context / history / untrusted-content だが、Ticket は explicit Tool operations、ordinary history、no hidden injection、bounded/rich content serialization を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WSN` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WSN)`: outgoing `depends_on 00001KVHR3WRY` is now closed。
- `TicketOrchestrationPlanQuery(00001KVHR3WSN)`: previous `blocked_by 00001KVHR3WRY` is resolved; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `6ac916c7`。
  - queued: `00001KVHR3WSN`, `00001KVHR3WSW`。
  - inprogress: 0。
  - spawned child implementation Pods: 0。
  - no matching MCP resources/prompts branch/worktree。

IntentPacket:

Intent:
- Expose MCP resources/prompts as explicit namespaced Yoi tool operations: `resources/list`, `resources/read`, `prompts/list`, `prompts/get`。
- Returned resources/prompt templates are untrusted Tool result data and must be recorded through ordinary Tool result/history paths。
- Do not inject resource/prompt content directly into model context outside Tool history。

Binding decisions / invariants:
- No hidden context injection path。
- All returned content/templates are untrusted data。
- Bound result sizes and rich/embedded content serialization。
- Handle pagination/list bounds where applicable。
- Diagnostics identify server/resource/prompt operation without leaking secrets。
- Do not implement MCP tool execution itself beyond existing completed support。
- Do not implement list_changed refresh, sampling, or elicitation in this Ticket。
- Preserve prior MCP lifecycle/tool-call redaction/bounds/ordinary Tool path behavior。

Requirements / acceptance criteria:
- `resources/list` and `resources/read` can be invoked as explicit tools。
- `prompts/list` and `prompts/get` can be invoked as explicit tools。
- Results are ordinary Tool results and history records。
- No hidden context injection path is introduced。
- Oversize/rich content is bounded。
- Tests cover list/read/get happy paths, untrusted content, bounds, and no hidden injection。

Escalate if:
- Existing ToolRegistry/provider path cannot represent resources/prompts operations without broad provider-substrate changes。
- MCP prompt message/content serialization requires product decision beyond safe bounded Tool result data。
- Hidden context injection would be needed to satisfy a feature expectation; that would violate Ticket intent and must stop。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T09:37:22Z from: queued to: inprogress reason: orchestrator_acceptance_mcp_resources_prompts_tools field: state -->

## State changed

User explicitly authorized parallel implementation when no blocker exists. Ticket body/thread, relation metadata, closed MCP lifecycle dependency, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded MCP resources/prompts context were checked. Dependency `00001KVHR3WRY` is closed, no inprogress/capacity blocker exists, no matching worktree/branch exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:38:10Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `b2b4764f ticket: accept mcp resources prompts tools`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHR3WSN-mcp-resources-prompts-tools` on branch `impl/00001KVHR3WSN-mcp-resources-prompts-tools` at `b2b4764f`。
- Spawned Coder Pod `yoi-coder-00001KVHR3WSN` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, hidden context injection, list_changed/sampling/elicitation scope creep, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:57:12Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHR3WSN`.

Implementation commit:
- `3a22360a mcp: expose resources prompts tools`

Changed areas reported:
- `crates/mcp/src/stdio.rs`:
  - Added typed MCP protocol structs/helpers for `resources/list`, `resources/read`, `prompts/list`, and `prompts/get`。
  - Added resource/prompt request/result models preserving untrusted server-owned fields as data。
- `crates/pod/src/feature/mcp.rs`:
  - Registers explicit namespaced MCP operation tools when server capabilities advertise resources/prompts:
    - `Mcp_<server>_resources_list`
    - `Mcp_<server>_resources_read`
    - `Mcp_<server>_prompts_list`
    - `Mcp_<server>_prompts_get`
  - Executes these through ordinary `Tool` path using `ToolOutput`。
  - Serializes resource/prompt content as bounded untrusted JSON tool-result data。
  - Bounds list items, resource contents, prompt messages, text fields, JSON depth/node count, rich embedded blobs/images/audio, and total output bytes。
  - Preserves existing MCP `tools/call` behavior and redacted diagnostics。
  - Does not add hidden context injection; prompt/resource data is not appended as user/system messages。

Tests reported:
- Operation tool naming/origin/schema。
- Discovery registers resource/prompt operations without requiring `tools` capability。
- `resources/list` and `resources/read` happy paths through ordinary tool output。
- `prompts/list` and `prompts/get` happy paths through ordinary tool output。
- Untrusted prompt/resource content remains data。
- Rich/oversize resource/prompt content is bounded/omitted/truncated。

Coder validation reported:
- `cargo test -p pod mcp::tests`: passed, 13 tests。
- `cargo check -p pod -p mcp`: passed。
- `cargo fmt --all --check`: passed。
- `git diff --check`: passed。
- `cargo test -p mcp`: passed, 12 stdio lifecycle tests。
- `nix build .#yoi --no-link`: passed; dirty-tree warning expected because validation ran before commit。

Known deferrals / notes:
- `list_changed` refresh remains deferred。
- Sampling/elicitation not implemented。
- MCP resources/prompts tools are registered from advertised server capabilities; unsupported capabilities are not exposed as model-visible tools。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `3a22360a`。
- Diff from acceptance `b2b4764f..HEAD` is one implementation commit touching 2 files, about 1225 insertions / 36 deletions。
- `git diff --check b2b4764f..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on explicit tool operations, ordinary Tool result/history path, no hidden context injection, untrusted/bounded content serialization, capability-gated registration, pagination/bounds, no list_changed/sampling/elicitation scope creep, and tests。

---
