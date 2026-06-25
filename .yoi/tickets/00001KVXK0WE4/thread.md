<!-- event: create author: "yoi ticket" at: 2026-06-24T19:51:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:55:30Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:55:30Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:12:03Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T06:22:18Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーから「続けて」と明示 follow-up があり、queued dependency chain の次 Ticket として再確認した。
- `00001KVXK0WE4` は host-owned WebSocket driver の concrete slice で、Discord protocol、full reconnect/resume policy、secret store/auth injection、browser-facing WebSocket API、WIT/PDK templates update は non-goal として分離されている。
- outgoing `depends_on` は `00001KVXK0WDQ` と `00001KVXK0WDX` だが、両方とも done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVXK0WEA` は WIT/PDK/templates update で、この Ticket 完了後に進めるべき後続であり、この Ticket の acceptance blocker ではない。
- bounded context check で current `crates/pod/src/feature/plugin.rs` に existing pull `host_api.websocket` helpers、WebSocket grants/allowlist、Plugin Service lifecycle / ingress queue、Service output command model が存在することを確認した。Ticket の残る不確実性は host-owned connection driver / frame-to-ingress / output command send integration の local implementation に閉じる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WDQ` と `00001KVXK0WDX` は done。routing 前 plan は historical blocked_by records 2 件で、prerequisites 完了により解消済み。accepted plan `orch-plan-20260625-062148-3` を記録済み。
- Related Tickets: `00001KVXK0WDQ` and `00001KVXK0WDX` are done.
- Code context: `crates/pod/src/feature/plugin.rs` の WebSocket allowlist/grants, existing pull API helpers, Plugin Service ingress queue, output command model。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Service Plugin に紐づく host-owned WebSocket connection driver を追加し、incoming text frames を Plugin ingress queue に event として enqueue し、Service output command の `websocket_send` を grant-check 後に Host が送信できるようにする。

Binding decisions / invariants:
- Connection ownership is Host-side. Plugin code does not run a long-lived recv loop as recommended service path.
- Existing pull `host_api.websocket.recv(timeout)` API may remain for compatibility/internal bounded use, but docs/recommended service integration path must move away from it.
- WebSocket target authority is manifest declaration + enabled grant / allowlist. Unauthorized target/send fails closed.
- v0 handles text frames. Binary application payload is unsupported/diagnostic. ping/pong/close handling should be bounded/control/diagnostic, not app payload.
- Incoming frames are converted to service ingress events using existing bounded queue and lifecycle diagnostics.
- Outgoing sends use Service output command path from `00001KVXK0WDX`; do not bypass its validation/grant boundary.
- Full reconnect/resume policy, Discord/Slack protocol logic, secret injection design, browser-facing WebSocket API, WIT/PDK/templates update are non-goals.

Requirements / acceptance criteria:
- Host-owned WebSocket driver can manage a connection associated with a Service Plugin.
- Incoming text frames enqueue Plugin ingress events.
- Close/error/reconnect-needed states become ingress events or status diagnostics.
- Plugin output command can trigger WebSocket text send after grant checks.
- Ungranted send / unauthorized target is fail-closed diagnostic.
- Connection status, last frame time, last error, queue drops, send failures are visible in diagnostics/status.
- Tests cover driver lifecycle, incoming frame to ingress, send command execution/rejection, close/error diagnostics, and frame kind handling.

Implementation latitude:
- Exact struct names and whether tests use mock WebSocket client/connection or local bounded fake are coder choices.
- v0 may model reconnect-needed as diagnostic instead of automatic reconnect.
- Keep integration in `crates/pod/src/feature/plugin.rs` unless a small helper module split is clearly cleaner.

Escalate if:
- Real network tests or external services are required.
- Secret store/auth injection becomes necessary.
- Durable cross-process queue or scheduler semantics are required.
- Implementing this requires WIT/PDK/template changes rather than Rust-side host runtime only.
- Browser-facing WebSocket API or protocol-specific Discord/Slack behavior is needed.

Validation:
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Focused WebSocket driver / service ingress tests during development.

Current code map:
- Primary: `crates/pod/src/feature/plugin.rs`。
- Secondary only if necessary: docs/development recommended path wording。
- Avoid: WIT/PDK/templates update (reserved for `00001KVXK0WEA`), protocol-specific integrations, full reconnect policy, secret store, browser WebSocket API。

Critical risks / reviewer focus:
- hidden ambient network authority or bypassing manifest/grant allowlist。
- pull `recv(timeout)` remaining as recommended long-lived service path。
- output command send bypassing Service command validation。
- unbounded reader task/queue or dropped diagnostics。
- tests that require real external network。
- scope creep into PDK/templates or protocol-specific behavior。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WE4-plugin-websocket-driver` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---
