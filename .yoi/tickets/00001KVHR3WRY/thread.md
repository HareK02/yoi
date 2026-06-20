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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T07:45:58Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHR3WRY`.

Implementation commit:
- `a114fa9d mcp: implement stdio lifecycle client`

Changed areas reported:
- Added new internal reusable crate `crates/mcp`。
- Implemented stdio MCP lifecycle foundation:
  - explicit resolved stdio server spec/config bridge from typed manifest MCP config。
  - Tokio child process spawn with `stdin` / `stdout` / `stderr`。
  - newline-delimited JSON-RPC request/response handling。
  - `initialize` capability negotiation。
  - `notifications/initialized`。
  - bounded stdout/stderr/protocol payload handling。
  - bounded/redacted stderr diagnostics。
  - server-name + phase-aware errors。
  - safe shutdown via stdin close, wait, terminate, and kill fallback。
  - fail-closed server-to-client request handling including sampling/elicitation。
- Added mock local stdio MCP server test binary scoped to `crates/mcp` tests。
- Updated workspace membership and `package.nix` cargo hash。

Coder validation reported:
- `cargo test -p mcp`: passed。
- `cargo check`: passed。
- `cargo fmt --check`: passed。
- `git diff --check`: passed。
- `nix build .#yoi --no-link`: passed。

Acceptance notes reported:
- Mock stdio MCP server initializes successfully。
- Initialize failure includes server name and `initialize` phase。
- stderr diagnostics are bounded and secret/env values are redacted。
- Shutdown is deterministic with terminate/kill fallback for uncooperative server。
- Client capabilities do not advertise sampling or elicitation。
- Server sampling requests receive JSON-RPC `-32601` fail-closed response。
- No ToolRegistry/tool/resource/prompt registration was added。

Known deferrals:
- No MCP tools/resources/prompts exposure。
- No remote MCP / Streamable HTTP / OAuth。
- Downstream list/call/resource/prompt support remains for later Tickets。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `a114fa9d`。
- Diff from acceptance `c0e760d7..HEAD` is one implementation commit touching 8 files, about 1369 insertions / 1 deletion。
- `git diff --check c0e760d7..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on process lifecycle cleanup, JSON-RPC framing/bounds, stdout/stderr handling, secret redaction, sampling/elicitation fail-closed, no ToolRegistry/resources/prompts exposure, test determinism, and package/Nix impact。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T07:46:38Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHR3WRY-r1` against implementation branch `impl/00001KVHR3WRY-mcp-stdio-lifecycle-client`。
- Review target commit: `a114fa9d mcp: implement stdio lifecycle client`。
- Review baseline: `c0e760d7`。
- Reviewer task focuses on process lifecycle cleanup, JSON-RPC framing/bounds, initialize negotiation, stderr diagnostics/redaction, sampling/elicitation fail-closed behavior, no ToolRegistry/resources/prompts exposure, crate layering, deterministic tests, and Nix/package impact。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVHR3WRY-r1 at: 2026-06-20T07:51:30Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket contract。
- Diff: `c0e760d7..a114fa9d`。
- 主な対象: `crates/mcp/src/stdio.rs`, `crates/mcp/src/lib.rs`, `crates/mcp/tests/stdio_lifecycle.rs`, `crates/mcp/tests/fixtures/mock_server.rs`, `crates/mcp/Cargo.toml`, root `Cargo.toml`, `Cargo.lock`, `package.nix`。
- ToolRegistry/resource/prompt registration leakage と remote MCP/OAuth/HTTP scope creep を確認。

良い点:
- stdio child spawn、newline JSON-RPC framing、initialize + initialized notification、bounded stdout/stderr handling、stderr-as-diagnostics、fail-closed server-to-client requests、shutdown/terminate/kill fallback、no model-visible registration surface は Ticket intent に概ね沿っている。

Blocking issue:
1. Resolved stdio server spec の derived `Debug` が secret/env values を leak できる。
   - `crates/mcp/src/stdio.rs`
   - `McpStdioServerSpec` derives `Debug`。
   - `env: BTreeMap<String, String>` に inherited/env/literal/secret-derived values が入る。
   - `Redactions` は stderr/protocol diagnostics には使われているが、resolved spec 自体を `{:?}` で format/log すると env values が plaintext で出る。
   - これは Ticket の secret/env redaction 要件に反する。

Required fix:
- `McpStdioServerSpec` から `Debug` を外す、または env values を出さない custom redacted `Debug` を実装する。
- custom `Debug` を残す場合は regression test を追加する。

Non-blocking concerns:
- Initialize response の `protocolVersion` compatibility enforcement は今後 tools/resources/prompts registration 前に判断が必要。Lifecycle slice としては follow-up で可。
- `Drop` cleanup は best-effort `start_kill` / `kill_on_drop`。Downstream code should prefer explicit `shutdown()`。

Reviewer validation:
- `git diff --check c0e760d7..HEAD`: passed。
- `cargo fmt --check`: passed。
- `cargo test -p mcp`: passed。
- `cargo check`: passed。
- `nix build .#yoi --no-link`: passed。
- `cargo tree -p mcp --depth 1`: inspected; no product façade dependency found。

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T07:55:55Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVHR3WRY`.

New fix commit:
- `f396e1a2 mcp: redact stdio server spec debug`

Branch commits now:
- `f396e1a2 mcp: redact stdio server spec debug`
- `a114fa9d mcp: implement stdio lifecycle client`

Fix summary:
- Removed derived `Debug` from `McpStdioServerSpec`。
- Added custom redacted `Debug` implementation。
- Debug output preserves useful non-sensitive fields: `name`, `command`, `args`, `cwd`, env keys, and redaction count。
- Debug output never prints resolved env values or redaction values。
- Added regression test verifying `format!("{:?}", spec)` does not contain literal/inherited/env-ref/secret-ref plaintext values while still showing non-sensitive metadata。

Coder validation reported as passing:
- `cargo fmt --check`
- `cargo test -p mcp`
- `cargo check`
- `git diff --check c0e760d7..HEAD`
- `cargo tree -p mcp --depth 1`
- `nix build .#yoi --no-link`

Remaining deferrals:
- Protocol-version compatibility enforcement before downstream tools/resources/prompts registration。
- Downstream callers should prefer explicit `shutdown()` over best-effort `Drop` cleanup。

Orchestrator evidence checked before r2 review:
- Implementation worktree is clean。
- HEAD is `f396e1a2`。
- Fix diff from `a114fa9d..HEAD` touches `crates/mcp/src/stdio.rs` and `crates/mcp/tests/stdio_lifecycle.rs`, about 47 insertions / 1 deletion。
- Overall `git diff --check c0e760d7..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification of the redacted `Debug` blocker and final acceptance criteria。

---

<!-- event: review author: yoi-reviewer-00001KVHR3WRY-r1 at: 2026-06-20T07:58:02Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket authority / Orchestrator IntentPacket。
- r2 fix diff: `a114fa9d..f396e1a2`。
- 全体実装 diff: `c0e760d7..f396e1a2`。
- 主な対象: `crates/mcp/src/stdio.rs`, `crates/mcp/tests/stdio_lifecycle.rs`, new crate/workspace/package changes, ToolRegistry/resource/prompt registration leakage の有無。

Previous blocker verification:
- `McpStdioServerSpec` no longer derives `Debug`; it is `Clone` only。
- Custom `fmt::Debug` now redacts env values and prints only non-sensitive metadata / env keys / `redaction_count`。
- `redactions` contents are not printed。
- Regression test confirms `format!("{spec:?}")` includes useful non-sensitive metadata and excludes plaintext env-like values。

Blocking issues: none。

Non-blocking concerns / follow-ups:
- None blocking for this Ticket。Implementation remains scoped to stdio lifecycle/protocol foundations and does not add ToolRegistry/resource/prompt registration or remote/OAuth surfaces。

Reviewer validation:
- `git diff --check c0e760d7..HEAD`: passed。
- `cargo fmt --check`: passed。
- `cargo test -p mcp`: passed; 5 tests passed including redacted Debug regression。
- `cargo check`: passed。
- `cargo tree -p mcp --depth 1`: inspected; dependencies limited to `libc`, `manifest`, `secrets`, `serde`, `serde_json`, `thiserror`, `tokio`; no product façade dependency。
- `nix build .#yoi --no-link`: passed。
- Additional grep found no added ToolRegistry/resource/prompt registration or remote/OAuth/HTTP scope creep。

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T07:59:04Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `9cf5344f merge: mcp stdio lifecycle client`
- Implementation commits included:
  - `a114fa9d mcp: implement stdio lifecycle client`
  - `f396e1a2 mcp: redact stdio server spec debug`

Reviewer outcome:
- r1 requested changes for `McpStdioServerSpec` derived `Debug` leaking resolved env/secret-derived values。
- Coder added custom redacted `Debug` and regression test。
- r2 approved with no blocking issues。

Orchestrator validation after merge passed:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp`
- `cargo check`
- `cargo tree -p mcp --depth 1`
- `nix build .#yoi --no-link`
- `nix path-info -S .#yoi`: `112615056`

Validation log:
- inline Bash output for final validation command。

Final state:
- Orchestrator worktree clean at `9cf5344f` after validation。
- Implementation worktree remains available for cleanup after Ticket completion records are committed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T07:59:10Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `9cf5344f`, r2 review approved, and final Orchestrator validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p mcp`, `cargo check`, `cargo tree -p mcp --depth 1`, and `nix build .#yoi --no-link`.

---
