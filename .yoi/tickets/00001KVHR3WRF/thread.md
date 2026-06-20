<!-- event: create author: "yoi ticket" at: 2026-06-20T05:30:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:58:46Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:00:44Z -->

## Decision

Routing decision: waiting_capacity

Panel Queue により routing 対象として確認したが、現在 `00001KVHKWNQS` が `inprogress` で Coder 作業中のため、この MCP foundation Ticket は実装開始せず queued のまま保持する。

Evidence:
- Ticket body / relations を確認。
- Orchestrator worktree は clean。
- queued は MCP chain とこの Ticket 群、inprogress は `00001KVHKWNQS` のみ。
- matching branch/worktree は存在しない。

Next:
- `00001KVHKWNQS` が完了して実装 capacity が空いた後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T06:56:16Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body は local stdio MCP server configuration / trust policy の範囲を、spawning/lifecycle を含まない config parse/validation/diagnostics Ticket として明確に定義している。
- 前回の waiting_capacity note は `00001KVHKWNQS` が inprogress だったためだが、現在 `00001KVHKWNQS` は closed で capacity blocker は解消済み。
- `00001KVHR3WRF` 自身には未解決 blocking relation はない。Incoming `00001KVHR3WRY depends_on this` は後続 Ticket であり blocker ではない。
- 現在 inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は mcp / config / trust-boundary / secrets / process-exec だが、Ticket は no process spawning、no auto-start、secret redaction、local executable trust boundary、Plugin permissions / `pod::feature` authority separation などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHR3WRF` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHR3WRF)`: no outgoing blocking dependency; incoming lifecycle Ticket depends on this。
- `TicketOrchestrationPlanQuery(00001KVHR3WRF)`: previous waiting capacity note resolved by `00001KVHKWNQS` closure; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `a5df9e37`。
  - queued: MCP chain remains queued。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching MCP implementation branch/worktree。

IntentPacket:

Intent:
- Add typed Profile/config support for named local stdio MCP servers and the trust-policy diagnostics around that config。
- This Ticket is intentionally config-only: parse, validate, redact, and document; do not spawn processes or implement JSON-RPC lifecycle。

Binding decisions / invariants:
- No package/workspace presence auto-start。Config alone must not spawn an MCP process。
- Local stdio MCP servers are local executables running with user OS permissions; Yoi feature authority does not sandbox OS-level side effects。
- Keep MCP config/trust policy separate from Plugin permissions and `pod::feature` authority。
- Config must be explicit and named; no implicit discovery or auto-enablement。
- Command/args/cwd/env/secret-related configuration must validate fail-closed with bounded diagnostics。
- Secret/env values must be redacted in diagnostics/log/model-facing surfaces; do not persist plaintext secrets through errors/debug output。
- This Ticket must not implement initialize/capability negotiation, tool/resource/prompt registration, streamable HTTP/OAuth/remote MCP, or subprocess spawning。

Requirements / acceptance criteria:
- A Profile/config can declare a named local stdio MCP server。
- Invalid command/env/secret config is rejected with bounded diagnostic。
- Secrets are not emitted in plaintext diagnostics/log/model context。
- Config alone does not spawn a process。
- Docs explain that configured local MCP servers are not OS-sandboxed by Yoi feature authority。
- Tests cover valid config, invalid config, secret redaction, and no auto-start。

Escalate if:
- Existing Profile/config model cannot represent MCP server declarations without a broader config/profile schema decision。
- Secret reference representation would require changing the local secret-store policy rather than referencing existing secret/env abstractions。
- Tests require actual process spawning; that belongs to a later Ticket, not this one。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating the implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T06:56:23Z from: queued to: inprogress reason: orchestrator_acceptance_mcp_stdio_config_trust field: state -->

## State changed

Ticket body/thread, relation metadata, resolved capacity blocker, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded MCP config/trust context were checked. There is no unresolved blocking dependency, no inprogress/capacity blocker, and no missing planning decision. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---
