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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T08:29:21Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHR3WS6`.

Implementation commit:
- `66fa9d55 mcp: register stdio server tools`

Changed areas reported:
- `crates/mcp`:
  - Added MCP `tools/list` protocol result/tool types。
  - Added `list_tools_page` and bounded paginated `list_tools_bounded`。
  - Added mock-server coverage for paginated `tools/list` and assertion that `tools/call` is not sent。
- `crates/pod`:
  - Added MCP stdio discovery feature module。
  - Resolves configured stdio servers, initializes them, calls bounded `tools/list`, normalizes discovered tools, and registers them through existing protocol-provider / ToolRegistry contribution paths。
  - Namespaces tools as stable names like `Mcp_<server>_<tool>`。
  - Rejects invalid schemas and duplicate normalized names with bounded diagnostics。
  - Ignores untrusted MCP metadata/annotations/instructions for authority purposes。
  - Registered tools are discovery-only and return explicit not-implemented error if invoked; no MCP `tools/call` execution is implemented in this Ticket。
- `package.nix` / `Cargo.lock`: updated for new `pod -> mcp` dependency and refreshed `cargoHash`。

Coder validation reported:
- `cargo test -p mcp list_tools --test stdio_lifecycle`
- `cargo test -p pod feature::mcp --lib`
- `cargo test -p mcp`
- `cargo fmt --check`
- `cargo check -p pod -p mcp`
- `git diff --check`
- `nix build .#yoi --no-link` after refreshing stale `cargoHash`。

Known risks / deferrals reported:
- MCP tool execution remains intentionally unimplemented; registered discovery-only stubs never send `tools/call`。
- Resources/prompts and `list_changed` handling are deferred。
- Secret-backed MCP stdio env resolution currently passes no Pod secret store from this integration path; non-secret stdio configs are supported by this Ticket。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `66fa9d55`。
- Diff from acceptance `a59e5c1e..HEAD` is one implementation commit touching 9 files, about 852 insertions / 4 deletions。
- `git diff --check a59e5c1e..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on ToolRegistry contribution path, schema/name normalization, no `tools/call`, discovery-only invocation behavior, metadata authority boundaries, secret-store deferral, and tests。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T08:30:22Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHR3WS6-r1` against implementation branch `impl/00001KVHR3WS6-mcp-tool-registration`。
- Review target commit: `66fa9d55 mcp: register stdio server tools`。
- Review baseline: `a59e5c1e`。
- Reviewer task focuses on normal ToolRegistry contribution path, untrusted metadata/schema/name normalization, no `tools/call`, discovery-only invocation behavior, no resources/prompts/list_changed registration, diagnostics bounds, secret-store deferral, and tests。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---
