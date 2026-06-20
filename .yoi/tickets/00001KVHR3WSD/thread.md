<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:59:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により routing 対象として確認したが、`00001KVHR3WSD` は `00001KVHR3WS6` に `depends_on` している。MCP `tools/call` execution は registered MCP tools を前提にするため、`00001KVHR3WS6` が closed になるまで実装開始せず queued のまま保持する。

Next:
- `00001KVHR3WS6` が closed になった後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T08:48:07Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- User directive: 「blocker無いなら並列にやっちゃえよ」。この指示により、未解決 blocker がない Ticket は並列実装対象として扱う。
- 前回は `00001KVHR3WS6` tool registration が未完了だったため blocked/queued hold としたが、現在 `00001KVHR3WS6` は closed。
- Ticket body は MCP `tools/call` execution、permission-before-call、ordinary Tool result/history path、normal result / MCP `isError` / JSON-RPC protocol error の区別、bounded result serialization を実装可能な粒度で定義している。
- 現在 inprogress は Dashboard/Console TUI refactor `00001KVHX0WBE` のみで、作業領域は TUI/CLI naming/module boundary。MCP `tools/call` 実装とは直接 conflict しないため、別 worktree / sibling Coder Pod で並列化できる。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk domain は mcp / tools-call / permission / history / bounded-output だが、Ticket は permission denied before server request、ordinary Tool result/history path、bounded output、untrusted content treatment を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WSD` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WSD)`: outgoing `depends_on 00001KVHR3WS6` is now closed。
- `TicketOrchestrationPlanQuery(00001KVHR3WSD)`: previous `blocked_by 00001KVHR3WS6` is resolved; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `381db88e`。
  - inprogress: `00001KVHX0WBE` only。
  - visible spawned child: Dashboard/Console Coder only。
  - no matching MCP tools-call branch/worktree。

IntentPacket:

Intent:
- Route invocation of registered MCP-backed Yoi tools to MCP `tools/call` through ordinary Yoi Tool execution/result/history paths。
- Enforce existing PreToolCall / Tool permission policy before any MCP server request is sent。

Binding decisions / invariants:
- Permission denial must occur before sending `tools/call` to the MCP server。
- MCP result content is untrusted and must not become hidden context injection。
- Results must be recorded through ordinary Tool call/result history path。
- Distinguish normal result, MCP `isError: true`, and JSON-RPC protocol error。
- Serialize content blocks / structuredContent / `_meta` boundedly; oversize/rich results must be truncated or rejected by explicit policy。
- Preserve lifecycle/registration redaction and bounds from previous MCP Tickets。
- Do not implement resources/read, prompts/get, list_changed, sampling, or elicitation in this Ticket。

Requirements / acceptance criteria:
- MCP mock tool returns normal result through ordinary Yoi Tool result。
- MCP `isError: true` is represented distinctly from JSON-RPC protocol failure。
- Permission denied call is not sent to MCP server。
- Oversize/rich results are bounded/truncated or rejected according to explicit policy。
- Tool history shows ordinary tool call/result, not hidden context injection。
- Tests cover normal result, `isError`, protocol error, permission denial, and output bounds。

Escalate if:
- Existing ToolRegistry contribution path cannot route invocation without broad provider-substrate redesign。
- Permission-before-call cannot be represented without changing Tool policy architecture。
- Result serialization requires product decisions beyond a safe bounded subset。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T08:48:15Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_mcp_tools_call field: state -->

## State changed

User explicitly authorized parallel implementation when no blocker exists. Ticket body/thread, relation metadata, closed MCP tool-registration dependency, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded MCP tools/call context were checked. Dependency `00001KVHR3WS6` is closed, no matching worktree/branch exists, and no missing planning decision was found. Accepting this queued Ticket for parallel implementation before worktree/Pod side effects.

---
