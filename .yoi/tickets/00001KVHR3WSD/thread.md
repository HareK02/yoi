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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T08:49:21Z -->

## Implementation report

Implementation orchestration start:
- User explicitly authorized parallel work for Tickets without blockers。
- Acceptance records were committed on Orchestrator branch: `92432ad7 ticket: accept mcp tools call`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHR3WSD-mcp-tools-call` on branch `impl/00001KVHR3WSD-mcp-tools-call` at `92432ad7`。
- Spawned Coder Pod `yoi-coder-00001KVHR3WSD` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, resources/prompts/list_changed scope creep, merge, Ticket close, and cleanup。

Next action:
- Let MCP tools/call Coder proceed in parallel with Dashboard/Console Coder. Inspect implementation reports and route each to Reviewer when ready。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:08:42Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHR3WSD`.

Implementation commit:
- `9a245403 mcp: execute stdio tool calls`

Changed areas reported:
- `crates/mcp/src/stdio.rs`:
  - Added typed `CallToolRequest`, `CallToolResult`, and `McpContentBlock`。
  - Added `McpStdioClient::call_tool(...)` for MCP `tools/call`。
- `crates/pod/src/feature/mcp.rs`:
  - Replaced discovery-only MCP tool stub with executable `McpStdioTool`。
  - Execution spawns/initializes the configured stdio MCP server, sends `tools/call`, then shuts down。
  - Result serialization is deterministic/model-visible/untrusted and bounded: content block cap, text/string truncation, JSON depth/node caps, binary/rich image/audio data omitted with size metadata, final output byte cap。
  - MCP `isError: true` is represented as an MCP tool-level result distinct from JSON-RPC protocol errors。
- `crates/mcp/tests/fixtures/mock_server.rs`:
  - Added mock modes for normal `tools/call`, MCP `isError`, JSON-RPC protocol error, and no-call assertion。
- `crates/mcp/tests/stdio_lifecycle.rs`:
  - Added focused lifecycle/client tests for normal result, `isError`, protocol error, and permission-denial-style no-call。

Coder validation reported:
- `cargo test -p mcp --test stdio_lifecycle`: passed, 12 tests。
- `cargo test -p pod feature::mcp`: passed, 9 tests。
- `cargo check -p mcp -p pod`: passed。
- `cargo fmt --check`: passed。
- `git diff --check`: passed。
- `nix build .#yoi --no-link`: passed。

Known deferrals:
- MCP resources/read, prompts/get, list_changed, sampling, and elicitation remain unimplemented as requested。
- MCP `isError: true` is returned through ordinary `ToolOutput` with explicit `status: "mcp_is_error"` / `isError: true`; JSON-RPC failures remain `ToolError`s。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `9a245403`。
- Diff from acceptance `92432ad7..HEAD` is one implementation commit touching 4 files, about 688 insertions / 11 deletions。
- `git diff --check 92432ad7..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on permission-before-call, ordinary Tool result/history path, `isError` vs protocol error distinction, output bounds/untrusted content, no resources/prompts/list_changed scope creep, and test coverage。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T09:09:50Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHR3WSD-r1` against implementation branch `impl/00001KVHR3WSD-mcp-tools-call`。
- Review target commit: `9a245403 mcp: execute stdio tool calls`。
- Review baseline: `92432ad7`。
- Reviewer task focuses on permission-before-call, ordinary Tool result/history path, `isError` vs protocol error semantics, output bounds/untrusted content, lifecycle redaction/shutdown preservation, no resources/prompts/list_changed/sampling/elicitation scope creep, tests, and package validation。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVHR3WSD-r1 at: 2026-06-20T09:16:53Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket contract / Orchestrator IntentPacket。
- Implementation diff: `92432ad7..9a245403`。
- 主な対象: `crates/pod/src/feature/mcp.rs`, `crates/mcp/src/stdio.rs`, `crates/mcp/tests/stdio_lifecycle.rs`, `crates/mcp/tests/fixtures/mock_server.rs`。
- Ordinary Worker/permission path: `crates/llm-worker/src/worker.rs`, `crates/pod/src/permission.rs`, `crates/pod/src/feature.rs`。
- Out-of-scope MCP surfaces (`resources/read`, `prompts/get`, `list_changed`, sampling/elicitation) の追加有無。

Blocking issues: none。

Critical risk checks:
- Permission denial occurs before `tools/call` can be sent:
  - MCP-discovered tools are ordinary `ToolDefinition`s wrapping `McpStdioTool`。
  - Protocol-provider contributions install through `register_tool_contribution`, not a private bypass。
  - Worker executes `pre_tool_call` first and only executes approved tools afterward。
  - Pod manifest permissions are `PreToolCall` hooks returning deny as synthetic results。
- Result flow uses ordinary Tool result/history path:
  - `McpStdioTool::execute` returns `ToolOutput`。
  - Worker converts to `ToolResult` and commits through existing history append path。
  - No hidden context injection or out-of-band model context path found。
- Normal MCP result / MCP `isError: true` / JSON-RPC protocol error are distinct:
  - `CallToolResult` carries `is_error` separately from JSON-RPC error。
  - MCP `isError: true` renders as model-visible untrusted data with `"status": "mcp_is_error"`。
  - JSON-RPC error becomes `ToolError::ExecutionFailed` with protocol error wording。
- Output bounding/untrusted handling exists for content blocks, text/string caps, JSON depth/node caps, rich image/audio data omission, and final byte cap。
- Result root is marked `untrusted_mcp_tools_call_result` and preserves server payload as data。
- Existing lifecycle/redaction/shutdown behavior is preserved。
- No resources/prompts/list_changed/sampling/elicitation scope creep found。

Non-blocking concerns / follow-ups:
- MCP-specific permission-denial test is lifecycle no-call style, while actual Worker deny/no-execute behavior is covered by generic Worker synthetic-result test path. Code inspection confirms MCP tools use ordinary Worker path, so this is not blocking. Future MCP-specific Worker-level regression would make the invariant more direct。
- Minor wording nit: successful `tools/call` with shutdown failure says “shutdown after tools/call failed”; functionally non-blocking。

Reviewer validation:
- `git diff --check 92432ad7..HEAD`: passed。
- `cargo fmt --check`: passed。
- `cargo test -p mcp --test stdio_lifecycle`: passed, 12 tests。
- `cargo test -p pod feature::mcp`: passed, 9 tests。
- `cargo check -p mcp -p pod`: passed。
- `nix build .#yoi --no-link` not run by Reviewer because diff has no Cargo/Nix/source-filter changes and focused Rust checks passed。

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:18:04Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `399a9d43 merge: mcp tools call`
- Implementation commit included:
  - `9a245403 mcp: execute stdio tool calls`

Reviewer outcome:
- r1 approved with no blocking issues。
- Reviewer confirmed permission-before-call through ordinary Worker PreToolCall path, ordinary Tool result/history path, distinct MCP `isError` vs JSON-RPC protocol error, bounded/untrusted result serialization, and no resources/prompts/list_changed/sampling/elicitation scope creep。

Orchestrator validation after merge passed:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp --test stdio_lifecycle`
- `cargo test -p pod feature::mcp`
- `cargo check -p mcp -p pod`
- `nix build .#yoi --no-link`
- `nix path-info -S .#yoi`: `113196368`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-lkjYsX.log`

Final state:
- Orchestrator worktree clean at `399a9d43` after validation。
- Implementation worktree remains available for cleanup after Ticket completion records are committed。
- Dashboard/Console review remains active in parallel and is unaffected by this merge。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T09:18:12Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `399a9d43`, review approved, and final Orchestrator validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p mcp --test stdio_lifecycle`, `cargo test -p pod feature::mcp`, `cargo check -p mcp -p pod`, and `nix build .#yoi --no-link`.

---

<!-- event: state_changed author: hare at: 2026-06-20T09:18:51Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-20T09:18:51Z status: closed -->

## 完了

## Resolution

`00001KVHR3WSD` を完了しました。

実装内容:
- MCP `tools/call` typed request/result/content types を追加しました。
- `McpStdioClient::call_tool(...)` を追加しました。
- MCP discovered tool の discovery-only stub を executable `McpStdioTool` に置き換えました。
- Execution は configured stdio MCP server を spawn/initialize し、`tools/call` を送信して shutdown します。
- Permission denial は ordinary Worker `PreToolCall` path により Tool execution 前に適用されるため、denied call は MCP server に送信されません。
- Results は ordinary Tool result/history path を通ります。Hidden context injection はありません。
- Normal MCP result、MCP `isError: true`、JSON-RPC protocol error を区別しました。
- MCP content / structuredContent / `_meta` / rich output は untrusted data として bounded に serialization されます。
- Image/audio data は raw payload を落とし、size metadata のみ残します。
- Resources/read、prompts/get、list_changed、sampling、elicitation は実装していません。

主な commit:
- `9a245403 mcp: execute stdio tool calls`
- `399a9d43 merge: mcp tools call`

Review:
- r1 は `approve`。
- Reviewer は permission-before-call、ordinary Tool result/history path、`isError` と protocol error の区別、bounded/untrusted result handling、out-of-scope surface が無いことを確認しました。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp --test stdio_lifecycle`
- `cargo test -p pod feature::mcp`
- `cargo check -p mcp -p pod`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113196368`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-lkjYsX.log`

---
