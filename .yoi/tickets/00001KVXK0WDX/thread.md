<!-- event: create author: "yoi ticket" at: 2026-06-24T19:51:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:55:29Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:55:29Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:12:02Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T05:46:02Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーから「続けて」と明示 follow-up があり、queued dependency chain の次 Ticket として再確認した。
- `00001KVXK0WDX` は Service output command model の concrete slice で、WebSocket transport、HTTP request dispatch completion、Domain operation command completion、WIT/PDK/templates update は non-goal として後続に分離されている。
- outgoing `depends_on` は `00001KVXK0WDQ` だが、`00001KVXK0WDQ` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVXK0WE4` は WebSocket driver slice で、この Ticket 完了後に進めるべき後続であり、この Ticket の acceptance blocker ではない。
- bounded context check で current Plugin service lifecycle / ingress queue implementation が `crates/pod/src/feature/plugin.rs` に入り、event dispatch/status diagnostics の拡張点が存在することを確認した。Ticket の残る不確実性は command envelope / validation / grant-check / diagnostics の local implementation に閉じる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WDQ` は done。routing 前 plan は historical blocked_by `00001KVXK0WDQ` のみで、prerequisite 完了により解消済み。accepted plan `orch-plan-20260625-054531-2` を記録済み。
- Related Tickets: `00001KVXK0WD3`, `00001KVXK0WDH`, `00001KVXK0WDQ` は done。
- Code context: `crates/pod/src/feature/plugin.rs` の Plugin Service lifecycle / bounded ingress queue / `PluginIngressEvent` / status diagnostics / component runtime。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Service Plugin ingress handler の戻り値として output command envelope を表現し、Host が command ごとに manifest declaration / enablement grant / runtime policy を fail-closed に検査し、結果を service diagnostics/status から追えるようにする。

Binding decisions / invariants:
- Service output command は Tool Plugin の ordinary `ToolOutput` path と型・処理経路・docs/tests で区別する。
- v0 command kind は最小集合に留める: diagnostic/status update, host request dispatch placeholder, websocket send placeholder。
- WebSocket send の実 transport 実装、HTTP request dispatch completion、Domain operation command completion は non-goal。
- Unsupported / ungranted / malformed command は実行せず typed diagnostic にする。
- Unrestricted shell / filesystem command や hidden LLM context injection は絶対に導入しない。
- Existing Plugin Service lifecycle / bounded ingress queue and Component Model-only runtime from prerequisites must not regress.

Requirements / acceptance criteria:
- `handle-ingress` / service event handler result can carry output command list.
- Command has correlation id / source event id / command id / kind / payload / requested_at.
- Host parses, validates, and grant-checks each command.
- Ungranted command is not executed and appears as typed diagnostic.
- Safe diagnostic/status update command is executed or recorded.
- WebSocket send / request dispatch placeholders are grant-checkable and safely unsupported without transport.
- Command execution result is visible from service status / diagnostics or run overview-equivalent state.
- Tests distinguish Tool Plugin output from Service output commands.

Implementation latitude:
- Exact Rust names/enums and JSON/envelope shape may follow existing plugin code style.
- Manifest declaration/grant mapping can be minimal v0 as long as it is explicit and fail-closed.
- Existing tests/fixtures may be extended in `crates/pod/src/feature/plugin.rs`; docs/comments may be updated if needed.

Escalate if:
- Implementing output commands requires actual WebSocket/HTTP transport.
- Current manifest/grant model cannot represent placeholder command grants without broader schema redesign.
- Command results must be persisted in a durable cross-process run overview to satisfy tests.
- Tool Plugin output must be routed through Service command processing.

Validation:
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Focused plugin service command tests during development are expected.

Current code map:
- Primary: `crates/pod/src/feature/plugin.rs`。
- Secondary only if necessary: manifest grant declarations/tests/docs comments。
- Avoid: WebSocket transport driver, HTTP request dispatch completion, WIT/PDK/templates service event update, durable cross-process queue, remote runtime protocol。

Critical risks / reviewer focus:
- output commands becoming ambient authority.
- ToolOutput and Service output commands being conflated.
- ungranted/malformed commands partially executing.
- placeholders accidentally performing network I/O。
- scope creep into WebSocket driver or PDK/WIT updates.

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDX-plugin-output-commands` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T05:46:44Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVXK0WDQ` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260625-054531-2` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDX-plugin-output-commands` を作成し、multi-agent-workflow に接続する。

---
