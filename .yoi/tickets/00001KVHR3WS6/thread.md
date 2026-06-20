<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:58:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により routing 対象として確認したが、`00001KVHR3WS6` は `00001KVHR3WRY` に `depends_on` している。MCP tools/list registration は initialized stdio lifecycle を前提にするため、`00001KVHR3WRY` が closed になるまで実装開始せず queued のまま保持する。

Next:
- `00001KVHR3WRY` が closed になった後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T08:00:45Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- 前回は `00001KVHR3WRY` stdio JSON-RPC lifecycle が未完了だったため blocked/queued hold としたが、現在 `00001KVHR3WRY` は closed。
- Ticket body は `tools/list` による registration scope、metadata/schema normalization、ToolRegistry integration、no `tools/call` execution、no resources/prompts を明確に分離している。
- 現在 inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は mcp / tools-list / ToolRegistry / schema / untrusted metadata だが、Ticket は server metadata を untrusted data として扱い、invalid schema / duplicate / collision を fail-closed、normal ToolRegistry pathを使い、no private MCP bypass / no `tools/call` during registration などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WS6` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WS6)`: outgoing `depends_on 00001KVHR3WRY` is now closed。Incoming `00001KVHR3WSD` / `00001KVHR3WSW` are downstream and not blockers。
- `TicketOrchestrationPlanQuery(00001KVHR3WS6)`: previous `blocked_by 00001KVHR3WRY` is resolved; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `68a8fc97`。
  - queued: `00001KVHR3WS6`, `00001KVHR3WSD`, `00001KVHR3WSN`, `00001KVHR3WSW`。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching MCP tool-registration branch/worktree。

IntentPacket:

Intent:
- Use the stdio MCP lifecycle client to call `tools/list` and register discovered MCP tools as ordinary Yoi model-visible tools through existing `pod::feature` / ToolRegistry contribution paths。
- This Ticket implements registration/discovery only. It must not send `tools/call`, execute MCP tools, or expose resources/prompts。

Binding decisions / invariants:
- Server-provided tool names, descriptions, schemas, annotations, and metadata are untrusted data。
- Normalize MCP tool names into stable namespaced Yoi tool names that include server namespace and avoid collisions。
- Validate/normalize descriptions and JSON schemas before ToolRegistry registration; invalid schemas/duplicates/collisions fail closed with bounded diagnostics。
- No server metadata may weaken Yoi instructions, scope, permissions, tool permissions, or system/developer instructions。
- Registration must go through normal ToolRegistry / `pod::feature` dynamic contribution path; no private MCP bypass。
- Do not send `tools/call` during registration。
- Do not register resources/prompts in this Ticket。
- Preserve lifecycle safety/redaction from `00001KVHR3WRY`。

Requirements / acceptance criteria:
- MCP mock server tool appears as model-visible Yoi tool with stable namespaced name。
- Invalid schema is rejected with bounded diagnostic。
- Duplicate/colliding names are rejected fail-closed。
- Server metadata cannot weaken Yoi instructions/scope/permissions。
- No `tools/call` request is sent during registration。
- Tests cover valid registration, pagination/bounds, invalid schema, duplicate/collision, and untrusted metadata normalization。

Escalate if:
- Existing `pod::feature` dynamic contribution API cannot register MCP tools without broader provider-substrate changes。
- Schema normalization requires product decisions beyond safe JSON schema subset / bounded diagnostics。
- ToolRegistry registration would force `tools/call` execution into this Ticket。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating the implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T08:00:53Z from: queued to: inprogress reason: orchestrator_acceptance_mcp_tool_registration field: state -->

## State changed

Ticket body/thread, relation metadata, closed MCP lifecycle dependency, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded ToolRegistry/schema context were checked. Dependency `00001KVHR3WRY` is closed, no inprogress/capacity blocker exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T08:01:48Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `a59e5c1e ticket: accept mcp tool registration`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHR3WS6-mcp-tool-registration` on branch `impl/00001KVHR3WS6-mcp-tool-registration` at `a59e5c1e`。
- Spawned Coder Pod `yoi-coder-00001KVHR3WS6` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, `tools/call`, resources/prompts exposure, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---
