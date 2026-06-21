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

<!-- event: review author: yoi-reviewer-00001KVMGAEJN-r1 at: 2026-06-21T12:43:06Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Diff `3eac7f8e..ce62d235` in worktree `/home/hare/Projects/yoi/.worktree/00001KVMGAEJN-plugin-websocket-host-api`:
  - `Cargo.lock`
  - `crates/manifest/src/plugin.rs`
  - `crates/pod/Cargo.toml`
  - `crates/pod/src/feature/plugin.rs`
  - `crates/pod/src/pod.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `docs/development/plugin-development.md`
  - `package.nix`
  - `resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit`
  - `resources/plugin/wit/yoi-plugin-instance-v1.wit`
  - `resources/plugin/wit/yoi-plugin-tool-v1.wit`
- Ticket authority:
  - `.yoi/tickets/00001KVMGAEJN/item.md`
  - `.yoi/tickets/00001KVMGAEJN/thread.md`

Blocking issue:

1. WebSocket open が実際には bounded / cancellable ではなく、open-connection limit も network I/O 後にしか適用されていない。

Evidence:
- `crates/pod/src/feature/plugin.rs:922-931`
  - `execute_plugin_websocket_open` は URL/grant validation 後に `client.open(...)` を呼び、その後で handle table に insert している。
- `crates/pod/src/feature/plugin.rs:2536-2558`
  - `TungstenitePluginWebSocketClient::open` は最初に `tungstenite::client::connect_with_config(...)` を呼ぶ。
  - `set_system_websocket_timeouts(...)` は connection established 後にしか呼ばれない。
- `crates/pod/src/feature/plugin.rs:2687-2698`
  - `PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS` は `PluginWebSocketHandleTable::insert` 内で check されるため、`client.open(...)` 完了後、つまり network I/O 後にしか適用されない。

Implications:
- DNS/TCP/TLS/WebSocket handshake が Yoi の WebSocket timeout 適用前に OS/library behavior に従って block し得る。
- `PLUGIN_WASM_TIMEOUT` wrapper は plugin Tool execution に timeout を返すが、spawned blocking task の open 自体は cancel しないため background で継続し得る。
- handle table が満杯でも、新しい outbound WebSocket connection を開いた後で reject するため、max open connections の resource-bound intent を破り、意図しない network side effect が発生し得る。

Why this blocks:
- Ticket は WebSocket API に bounded handle lifetime、max open connections、timeout/cancellation behavior、cleanup を要求している。
- capacity / timeout / cancellation の enforcement が network I/O 前に fail closed していないため、この acceptance criteria を満たしていない。

Required fix:
- Network I/O 前に capacity を reserve/check する。
- Open path を bounded/cancellable にする、または bound を保証できない場合は dialing 前に fail closed する。
- Timeout/cancellation/resource-bound semantics を tests で固定する。

Non-blocking concerns:
- WebSocket host API は component/tool WIT surface に追加され、raw helper path tests はあるが、「long-lived WebSocket connection を Tool call 内に隠さず Plugin instance/Service lifecycle として扱う」という Ticket invariant を pin する integration test/diagnostic は見当たらなかった。現行 Plugin instance が Tool/Service state を共有するなら許容可能だが、lifecycle boundary の regression test/diagnostic があるとより明確。
- `close_all` / drop cleanup と component `stop` cleanup はあるが、active blocking host calls は blocking operation が戻るまで handle-table mutex を保持し得る。これは open timeout/cancellation blocker の副次的問題。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check 3eac7f8e..HEAD`: passed
- `cargo test -p manifest websocket`: passed
- `cargo test -p pod websocket`: passed, 6 focused pod WebSocket tests
- `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`: passed
- `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed; reviewer did not reproduce earlier disk-space failure。
- Boundary/stale grep over docs/resources/manifest/pod/yoi CLI for `host_api.request`, `host_api.websocket`, WebSocket/SSE, hidden context/history, Dashboard/Ticket/Discord/reconnect terms found no hidden context injection, Ticket mutation, Dashboard channel, reconnect daemon, or Discord-specific overreach in inspected changes。

Worktree status:
- Clean on `impl/00001KVMGAEJN-plugin-websocket-host-api`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T13:16:05Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVMGAEJN`.

New fix commit:
- `a766048f fix: bound plugin websocket open`

Branch commits now:
- `4c1b8c3d feat: add plugin websocket host api`
- `ce62d235 chore: keep plugin websocket branch code-only`
- `a766048f fix: bound plugin websocket open`

Fix summary:
- `crates/pod/src/feature/plugin.rs`
  - Added pre-dial WebSocket capacity reservation / permit flow。
  - Max-open-connections now fails before invoking network client/open path。
  - Reservation is released on open failure。
  - Replaced sync `tungstenite` open path with bounded async `tokio-tungstenite` open under `tokio::time::timeout`。
  - Added deterministic fail-closed abstraction for clients that cannot guarantee bounded/cancellable open。
  - Added focused tests for max-open pre-dial rejection, open failure reservation cleanup, and unbounded open client fail-closed before dialing。
- `crates/pod/Cargo.toml` / `Cargo.lock`
  - Added `tokio-tungstenite` and `futures-util`。
- `package.nix`
  - Updated `cargoHash` to `sha256-cZxkmM42kbDp1Rv9gn4sCD5WIQLc0wCbjj4GbKjuA9Q=`。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p pod websocket`: passed
- `cargo test -p manifest websocket`: passed
- `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`: passed
- `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `git diff --check 3eac7f8e..HEAD`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVMGAEJN-plugin-websocket-host-api` at `a766048f`。
- r1-fix diff `ce62d235..HEAD` touches only `Cargo.lock`, `crates/pod/Cargo.toml`, `crates/pod/src/feature/plugin.rs`, and `package.nix`。
- `git diff --name-status 3eac7f8e..HEAD -- .yoi/tickets/00001KVMGAEJN`: no output。
- `git diff --check 3eac7f8e..HEAD`: passed。

Next action:
- Route back to Reviewer for r2 verification of WebSocket open capacity/timeout/cancellation bounds and final acceptance。

---

<!-- event: review author: yoi-reviewer-00001KVMGAEJN-r1 at: 2026-06-21T13:20:53Z status: approve -->

## Review: approve

Verdict: `approve`

r1-fix diff inspected:
- Reviewed `ce62d235..a766048f`:
  - `Cargo.lock`
  - `crates/pod/Cargo.toml`
  - `crates/pod/src/feature/plugin.rs`
  - `package.nix`
- Also re-checked overall implementation diff `3eac7f8e..a766048f` against Ticket authority and previous r1 blocker。

Previous blocker verification:
- Resolved。

Evidence in `crates/pod/src/feature/plugin.rs`:
- `execute_plugin_websocket_open` now:
  - validates URL / manifest / grant before dialing。
  - fails closed if selected client cannot guarantee bounded open。
  - reserves WebSocket capacity before `client.open(...)`。
  - commits reservation only after successful open。
- `PluginWebSocketHandles` / `PluginWebSocketHandleTable` now track reserved slots separately from active handles, so `PLUGIN_WEBSOCKET_MAX_OPEN_CONNECTIONS` rejects before network I/O。
- Reservation cleanup is RAII-style:
  - failed open drops the reservation。
  - failed commit also drops/releases reservation。
- `TungstenitePluginWebSocketClient` now uses `tokio-tungstenite` async connect under `tokio::time::timeout(...)`, replacing previous synchronous `tungstenite::client::connect_with_config(...)` path。
- Focused tests cover:
  - max-open rejecting before network open。
  - open failure releasing reservation。
  - unbounded clients failing closed before dialing。

Overall Ticket fit:
- `host_api.websocket` remains separate from `host_api.request`。
- Manifest `[[websocket]]` targets and `grants.websocket` remain independent from request targets/grants。
- Runtime connect requires both manifest target and grant before network I/O。
- URL matching covers scheme, host, port, and path prefix。
- WebSocket API is text-only, explicit receive only, and does not inject incoming messages into hidden context/history。
- Guest arbitrary handshake headers / embedded credentials remain rejected。
- Request API continues to reject WebSocket/SSE/persistent request attempts。
- Static inspection / `yoi plugin show` diagnostics distinguish request and WebSocket grant/target states。
- No Ticket mutation, Dashboard channel, scheduler/reconnect daemon, direct model Tool call, or Discord-specific overreach found in reviewed branch。

Blocking issues:
- None。

Non-blocking concerns:
- None requiring changes。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check 3eac7f8e..HEAD`: passed
- `cargo test -p pod websocket`: passed
- `cargo test -p manifest websocket`: passed
- `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`: passed
- `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `nix build .#yoi --no-link`: passed; disk-space failure did not reproduce。

Worktree status:
- Clean on `impl/00001KVMGAEJN-plugin-websocket-host-api` at `a766048f`。
- Branch diff no longer includes `.yoi/tickets/00001KVMGAEJN/*`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T13:26:05Z -->

## Implementation report

Reviewer r2 approvalを受けて、実装ブランチを Orchestrator worktree の `orchestration` branch に統合した。

統合:
- Merge commit: `354f1e10 merge: plugin websocket host api`
- Merged branch: `impl/00001KVMGAEJN-plugin-websocket-host-api`
- Implementation commits:
  - `4c1b8c3d feat: add plugin websocket host api`
  - `ce62d235 chore: keep plugin websocket branch code-only`
  - `a766048f fix: bound plugin websocket open`

Orchestrator worktree validation:
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p pod websocket`: passed
- `cargo test -p manifest websocket`: passed
- `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`: passed
- `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`: passed
- `cargo check -p manifest -p pod -p yoi`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

Final Orchestrator worktree status after validation: clean on `orchestration` at `354f1e10`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T13:26:12Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval、Orchestrator worktree への統合、focused manifest/pod/yoi plugin tests、cargo check、Ticket doctor、Nix build が完了したため `done` に遷移する。

---

<!-- event: state_changed author: hare at: 2026-06-21T13:26:27Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-21T13:26:27Z status: closed -->

## 完了

URL permission based Plugin WebSocket host API を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- `host_api.websocket` を `host_api.request` とは別 capability として追加。
- Manifest `[[websocket]]` target declaration と enablement `grants.websocket` を追加し、request targets/grants とは独立させた。
- Static inspection / `yoi plugin show` が WebSocket requested/granted/missing/grant-only/broad diagnostics を request diagnostics とは別に表示するようにした。
- Runtime connect は manifest target と enablement grant の両方が URL を許可する場合のみ network I/O に進む。
- URL checks cover scheme (`ws`/`wss`), host, port, and path prefix。
- Local/private/loopback WebSocket targets は ambient ではなく、明示 declaration + grant が必要。
- Host-owned WebSocket handle API を追加: open, send_text / send-text, recv, close。
- Text-only / explicit bounded receive とし、binary receive は fail closed / unsupported。
- Guest arbitrary handshake headers / embedded credentials を reject。
- Request API は WebSocket/SSE/persistent attempts を引き続き reject。
- Open path は pre-dial capacity reservation と bounded async `tokio-tungstenite` open under `tokio::time::timeout` により max-open / timeout semantics を network I/O 前から enforce。
- Reservation cleanup on open failure / failed commit を追加。
- WIT resource `yoi:host/websocket@1.0.0` と docs を更新。
- `tungstenite`, `tokio-tungstenite`, `futures-util` dependencies と `Cargo.lock` / `package.nix` cargo hash を更新。

統合・検証:
- Merge commit: `354f1e10 merge: plugin websocket host api`
- Implementation commits: `4c1b8c3d`, `ce62d235`, `a766048f`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p pod websocket`, `cargo test -p manifest websocket`, `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`, `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`, `cargo check -p manifest -p pod -p yoi`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Discord bridge 本体は実装していない。
- Reconnect/backoff/heartbeat scheduler、hidden context/history injection、Dashboard channel、Ticket mutation、direct model Tool invocation は追加していない。
- SecretRef-based credential injection は future follow-up。

---
