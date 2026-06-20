<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:58:54Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により routing 対象として確認したが、`00001KVHR3WRY` は `00001KVHR3WRF` に `depends_on` している。`00001KVHR3WRF` はまだ queued で未実装のため、この Ticket は実装開始せず queued のまま保持する。

Next:
- `00001KVHR3WRF` が closed になった後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T07:29:56Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- 前回は `00001KVHR3WRF` local stdio config/trust policy が未完了だったため blocked/queued hold としたが、現在 `00001KVHR3WRF` は closed。
- Ticket body は stdio JSON-RPC lifecycle client の scope を initialize/capability negotiation/shutdown/process diagnostics foundation として明確に定義し、ToolRegistry/tools/resources/prompts exposure は non-goal として分離している。
- 現在 inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は mcp / stdio / json-rpc / process-lifecycle / diagnostics だが、Ticket は explicit config only、bounded stderr diagnostics、safe shutdown/kill fallback、sampling/elicitation not advertised、no tools/resources/prompts registration などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WRY` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WRY)`: outgoing `depends_on 00001KVHR3WRF` is now closed。Incoming `00001KVHR3WS6` / `00001KVHR3WSN` are downstream and not blockers。
- `TicketOrchestrationPlanQuery(00001KVHR3WRY)`: previous `blocked_by 00001KVHR3WRF` is resolved; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `8f5eef94`。
  - queued: remaining MCP chain Tickets。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching MCP lifecycle branch/worktree。

IntentPacket:

Intent:
- Implement a local stdio MCP lifecycle client foundation that can spawn an explicitly configured local server, exchange newline-delimited JSON-RPC on stdin/stdout, run initialize/capability negotiation, send `notifications/initialized`, and shut down safely。
- This Ticket must not expose MCP tools/resources/prompts to the model-visible ToolRegistry; it only provides the lifecycle/protocol substrate for later Tickets。

Binding decisions / invariants:
- Only explicit named config from `00001KVHR3WRF` can be used; no implicit discovery or auto-start from workspace/package presence。
- Treat stdout as protocol messages and stderr as bounded diagnostics/logging, not automatic protocol failure。
- Bound stdout/stderr lines, diagnostics, protocol payloads, startup timeout, request timeout, shutdown timeout, and kill fallback。
- Include server name and startup phase in errors/diagnostics。
- Do not advertise sampling or elicitation client capabilities; if server requests them, fail closed / return an error as appropriate。
- Do not register tools/resources/prompts or expose any ToolRegistry surface in this Ticket。
- No remote MCP / Streamable HTTP / OAuth。
- Preserve trust boundary: local stdio server process runs as user OS executable; config validation is not sandbox authority。
- Secret/env values from config must remain redacted in diagnostics/logs/errors。

Requirements / acceptance criteria:
- Mock local stdio MCP server initializes successfully。
- Initialize failure reports server name and phase。
- Stderr is bounded and redacted where needed。
- Shutdown is safe and deterministic, with terminate/kill fallback。
- Sampling/elicitation are not advertised and fail closed if requested。
- No tools/resources/prompts are registered by this Ticket。
- Tests use local mock MCP server processes and cover success/failure/shutdown/diagnostic paths。

Escalate if:
- Existing runtime boundaries make it unclear where lifecycle client should live without dragging ToolRegistry or Plugin feature authority into the config layer。
- Protocol message typing requires a broad MCP provider substrate redesign。
- Deterministic process tests are infeasible without an explicit test harness decision。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating the implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T07:30:03Z from: queued to: inprogress reason: orchestrator_acceptance_mcp_stdio_lifecycle_client field: state -->

## State changed

Ticket body/thread, relation metadata, closed MCP config dependency, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded MCP lifecycle context were checked. Dependency `00001KVHR3WRF` is closed, no inprogress/capacity blocker exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T07:30:59Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `c0e760d7 ticket: accept mcp stdio lifecycle client`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHR3WRY-mcp-stdio-lifecycle-client` on branch `impl/00001KVHR3WRY-mcp-stdio-lifecycle-client` at `c0e760d7`。
- Spawned Coder Pod `yoi-coder-00001KVHR3WRY` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, ToolRegistry/tools/resources/prompts exposure for this Ticket, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---
