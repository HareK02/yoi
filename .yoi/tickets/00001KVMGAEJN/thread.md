<!-- event: create author: LocalTicketBackend at: 2026-06-21T07:11:34Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-21T07:14:05Z -->

## Intake summary

ユーザー指示により、この Ticket を Orchestrator が routing できる `ready` 状態にする。readiness は引き続き `requirements_sync_needed` であり、実装直行ではなく WebSocket capability の設計同期・仕様化として扱う。URL 権限を前提にし、`host_api.request` とは分離する binding decisions は Ticket body に記録済み。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-21T07:14:05Z from: planning to: ready reason: user_requested_ready field: state -->

## State changed

ユーザーから「readyにして」と明示されたため `planning` から `ready` へ遷移する。未決定点は残っているため、後続 Orchestrator routing では requirements/design sync として扱う。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T07:15:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T07:17:40Z -->

## Decision

Routing decision: `return_to_planning_requirements_sync`

Reason:
- Ticket frontmatter has `readiness: requirements_sync_needed`。
- Intake summary explicitly says this should be handled as WebSocket capability design/spec sync, not implementation-direct routing。
- Ticket body still lists concrete open questions that must be resolved before implementation or worktree side effects:
  - WebSocket connection ownership: Yoi host-owned vs Plugin-instance-owned host API。
  - Incoming message dispatch route: Ingress vs Service event/status stream vs other host routing。
  - Reconnect / backoff / heartbeat / shutdown / cancellation / restore scope for first slice。
  - Auth/headers/secrets model and grant/display split。
  - Whether this work item closes as design/spec only or includes a minimal implementation slice。
- Related Ticket `00001KVMG8FTW` is now routed for implementation of the `host_api.request` URL permission model. WebSocket design should reuse or intentionally diverge from that resulting URL permission schema after it stabilizes。

Decision:
- Do not create implementation worktree or spawn Coder/Reviewer for this Ticket now。
- Return to `planning` with concrete missing requirements/design decisions。
- Record relation to `00001KVMG8FTW` so WebSocket design can be resumed after request permission model lands。

Required next planning output before queueing again:
- Decide whether this is design/spec only or includes a minimal implementation slice。
- Decide connection ownership and lifecycle boundary。
- Decide incoming-message durable/visible dispatch path and no-hidden-context behavior。
- Decide WebSocket URL permission/grant schema relative to `host_api.request` request-target schema。
- Decide auth/secrets display/grant model and non-goals for reconnect/backoff/heartbeat in first slice。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T07:17:52Z from: queued to: planning reason: requirements_sync_needed_connection_lifecycle_and_scope_decisions field: state -->

## State changed

Ticket remains `requirements_sync_needed` and has unresolved design questions around connection ownership, incoming-message dispatch path, lifecycle bounds, auth/secrets handling, and design-vs-implementation scope. Returning to planning rather than starting implementation side effects.

---

<!-- event: intake_summary author: hare at: 2026-06-21T11:01:20Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T11:01:20Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T11:34:07Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T11:35:44Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket was previously returned to planning because it needed concrete WebSocket connection ownership / lifecycle / permission model decisions。
- Current Ticket body is now retitled and refined as implementation work: `Plugin: URL 権限ベースの WebSocket host API を実装する`。
- It defines explicit `host_api.websocket` semantics, URL permission/grant model, lifecycle/bounds/non-goals, acceptance criteria, tests, docs, and validation。
- Recorded dependency `00001KVMGAEJN depends_on 00001KVMG8FTW` is now satisfied because `00001KVMG8FTW` is `closed` and `host_api.request` URL permission model is integrated。
- Relations / orchestration plan have no remaining blockers。
- Active `00001KVMT2J25` work is protocol/pod/TUI reconnect snapshot and is semantically separate from Plugin WebSocket host API; parallel implementation is acceptable。
- Orchestrator worktree is clean on `orchestration` at `d8027781` before routing side effects; target worktree / branch is not present。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct `item.md` read。
- `TicketRelationQuery(00001KVMGAEJN)`: one `depends_on 00001KVMG8FTW`, target Ticket is `closed`。
- `TicketOrchestrationPlanQuery(00001KVMGAEJN)`: no records。
- `TicketList(state=queued)`: this Ticket is the only queued Ticket。
- `ListPods`: only active child for other work is `yoi-reviewer-00001KVMT2J25-r1`。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `crates/manifest/src/plugin.rs` now has `host_api.request`, `PluginRequestGrant`, and manifest request target schema。
  - `crates/pod/src/feature/plugin.rs` has `PluginRequestClient`, `validate_plugin_request_request`, request allowlist inspection, and explicit WebSocket rejection in request path。
  - No existing tungstenite/tokio-tungstenite/websocket dependency found in Cargo manifests。
  - Docs currently state WebSocket/persistent transports require a separate Plugin capability。

IntentPacket:

Intent:
- Add a separate URL-permission-based Plugin WebSocket host API, not an extension of `host_api.request`, suitable as a foundation for Discord/gateway-like integrations without implementing Discord itself。

Binding decisions / invariants:
- API name is `host_api.websocket`; do not fold WebSocket into `host_api.request`。
- URL permission model should mirror/reuse the `host_api.request` target/grant review semantics where sensible, while keeping websocket-specific lifecycle/bounds explicit。
- Authority requires both manifest-declared WebSocket target and enablement grant before opening a connection。
- WebSocket connection is host-owned and Plugin-driven: guest requests open/send/recv/close via host API, but host enforces handles, bounds, timeouts, and shutdown cleanup。
- No ambient network/socket access, no raw WASI sockets, no arbitrary URL by default。
- Secrets/auth headers are not solved by guest-memory arbitrary credential headers; keep credential-bearing header policy conservative and explicit。
- Incoming messages from WebSocket are delivered to the guest through explicit host API return values or bounded polling/receive operations, not hidden model context injection。
- No direct model Tool calls, Ticket mutation, Dashboard UI channel, or hidden history/context mutation。
- `host_api.request` must keep rejecting WebSocket/SSE/persistent connection attempts。
- First slice should avoid full background daemon scheduler unless it is minimal and bounded; preserve instance lifecycle cleanup。

Requirements / acceptance criteria:
- Manifest can declare WebSocket targets independently from request targets。
- Enablement config can grant WebSocket targets independently from request grants。
- Static inspection / `yoi plugin show` reports WebSocket requested/granted/missing/broad diagnostics separately from request。
- Runtime refuses connect unless manifest target and grant both allow the URL。
- URL checks cover scheme (`ws`/`wss`), host, port, path prefix, and any method/protocol constraints chosen for handshake。
- Local/private/loopback WebSocket targets require explicit declaration+grant。
- WebSocket API has bounded handle lifetime, max frame/message size, max open connections per Plugin instance, timeout/cancellation behavior, and cleanup on instance stop/trap/drop。
- Send/receive operations are bounded and typed; binary/text behavior is documented。
- Credential-like headers are rejected or explicitly not supported until SecretRef/grants exist。
- Tests cover allow/deny, grant-only/missing-grant, loopback allow/deny, broad diagnostics, request API still rejecting WebSocket, bounds/cleanup, and no hidden context mutation。

Implementation latitude:
- Rust dependency choice is Coder’s decision, e.g. `tokio-tungstenite` if suitable, but dependency/package/Nix implications must be handled。
- WIT/API shape can be handle-based with `open`, `send_text`/`send_binary`, `recv`, `close`, or similar. Keep it minimal and reviewable。
- If a fully live network integration test is hard, use local test server / mock client abstraction to validate runtime policy and handle lifecycle。
- Reuse request target/grant matching helpers where appropriate, but avoid overgeneralizing if it obscures WebSocket semantics。

Escalate if:
- Implementing safe host-owned connection handles requires a broader Plugin service/instance actor redesign than current runtime supports。
- SecretRef/auth header requirements become necessary for useful functionality。
- Background receive delivery requires hidden context/history mutation or direct model Tool invocation。
- Cargo/Nix dependency updates become blocked by unavailable crates or source filtering。
- Public API compatibility for new WIT world/import names becomes unclear。

Validation plan:
- `cargo fmt --check`
- Focused manifest tests for websocket permission/target/grant parsing and independence from request。
- Focused pod plugin tests for connection allow/deny policy, loopback explicit grants, grant-only/missing-grant, bounds, handle cleanup, and request API rejection of WebSocket。
- Focused yoi plugin CLI tests for WebSocket diagnostics。
- `cargo check` for affected crates。
- `git diff --check`
- stale/boundary grep to ensure request docs still reject WebSocket and WebSocket docs do not claim hidden event injection。
- `yoi ticket doctor`
- `nix build .#yoi --no-link` if dependencies/package/source-filter changed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T11:35:51Z from: queued to: inprogress reason: human_authorized_dependency_satisfied_websocket_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Previously recorded dependency on `00001KVMG8FTW` is now satisfied because `host_api.request` is closed/integrated, and this Ticket now has implementation-ready WebSocket host API requirements. Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T11:37:01Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVMGAEJN-plugin-websocket-host-api`
- Created branch:
  - `impl/00001KVMGAEJN-plugin-websocket-host-api`
- Base commit:
  - `3eac7f8e ticket: accept plugin websocket host api`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVMGAEJN`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVMGAEJN-plugin-websocket-host-api`

Parallelization note:
- `00001KVMT2J25` remains active in review and targets protocol/pod/TUI reconnect snapshot semantics. This Ticket should stay limited to Plugin WebSocket host API / manifest / runtime / CLI / docs / tests。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T12:20:55Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVMGAEJN`.

Implementation commit reported:
- `4c1b8c3d feat: add plugin websocket host api`

Changed areas reported:
- Manifest/config schema:
  - Added independent `host_api.websocket` capability。
  - Added manifest `[[websocket]]` targets and enablement `grants.websocket`。
- Runtime:
  - Added host-owned WebSocket handle lifecycle for raw Wasm and component imports。
  - Added `open`, `send_text` / `send-text`, `recv`, and `close`。
  - Enforces manifest+grant URL allowlist before network I/O。
  - Bounds open descriptor size, text/message size, max open connections, recv timeout, handle lifetime, and cleanup。
  - Rejects embedded credentials, guest handshake headers, unsupported subprotocols, non-`ws`/`wss` schemes, and binary message receive。
- Static inspection / CLI:
  - Reports WebSocket requested/granted/missing/grant-only/broad diagnostics separately from request diagnostics。
  - `yoi plugin show` now includes `configured_websocket_grants`。
- Docs/WIT:
  - Added `yoi:host/websocket@1.0.0` WIT interface。
  - Documented text-only API, explicit bounded receive, no hidden context/history injection, and future SecretRef credential follow-up。
- Dependency/package:
  - Added `tungstenite` to `crates/pod`。
  - Updated `Cargo.lock`。
  - Updated `package.nix` cargo hash to `sha256-TZrw6nJclXVRpFIUlYvimGTDXlxBMaQt6oM5C5DIGIU=`。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p manifest websocket`: passed
- `cargo test -p pod websocket`: passed
- `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`: passed
- `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `git diff --check`: passed
- stale/boundary grep for request/WebSocket docs and hidden context claims: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)

Nix status reported:
- First `nix build .#yoi --no-link` exposed expected cargo hash mismatch; Coder updated `package.nix`。
- After hash update, build advanced into dependency compilation then failed in sandbox compiling `aws-lc-sys` with `No space left on device`。
- Coder treated this as environmental disk-space failure, not Rust/package hash failure。

Orchestrator evidence checked:
- Worktree clean at `4c1b8c3d`。
- Diff `3eac7f8e..HEAD` includes expected code/docs/package areas but also includes child worktree edits to `.yoi/tickets/00001KVMGAEJN/{item.md,thread.md}`。
- Because Orchestrator owns durable Ticket records in this workflow, Coder was asked to remove those `.yoi/tickets` edits from the implementation branch before Reviewer routing。

Next action:
- Wait for Coder hygiene fix, then route code-only implementation diff to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T12:32:21Z -->

## Implementation report

Coder hygiene fix received from `yoi-coder-00001KVMGAEJN`.

New commit:
- `ce62d235 chore: keep plugin websocket branch code-only`

Result:
- Implementation branch now contains code/docs/package changes only; child-worktree Ticket record edits were removed from the branch diff。
- `git diff --name-status 3eac7f8e..HEAD -- .yoi/tickets/00001KVMGAEJN`: no output。
- Worktree clean at `ce62d235`。
- Diff `3eac7f8e..HEAD` now touches 11 expected files: Cargo/package, manifest/pod/yoi plugin CLI, docs, and WIT resources。
- `git diff --check 3eac7f8e..HEAD`: passed。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-21T12:33:03Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVMGAEJN-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVMGAEJN-plugin-websocket-host-api`
- Branch: `impl/00001KVMGAEJN-plugin-websocket-host-api`
- Baseline: `3eac7f8e`
- Implementation commits:
  - `4c1b8c3d feat: add plugin websocket host api`
  - `ce62d235 chore: keep plugin websocket branch code-only`
- Diff: `3eac7f8e..ce62d235`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- `host_api.websocket` remains distinct from `host_api.request`。
- Manifest target + enablement grant are both required before opening a connection。
- Host-owned WebSocket handle lifecycle is bounded and cleaned up。
- Credential/header, binary/text, local/private/loopback, broad diagnostics, and no-hidden-context semantics are safe。
- Request API still rejects WebSocket/SSE/persistent attempts。
- Cargo/package/Nix changes are sound; Coder reported Nix advanced past hash check then failed due `No space left on device` in `aws-lc-sys` build。

Orchestrator will wait for reviewer verdict before integration。

---
