<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:34:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:34:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:42:14Z from: ready to: planning reason: cli_state field: state -->

## State changed

State changed to `planning`.


---

<!-- event: decision author: hare at: 2026-06-25T16:42:14Z -->

## Decision

Returned to planning because the current ticket is not concrete enough.

The purpose is specifically observation: Backend subscribes to a Runtime-owned WebSocket stream to receive Worker output and related runtime/worker events. It is not a command channel, not browser-facing, and not the path for sending user input.

Before this can be ready, define the event model and protocol boundary concretely:
- which Worker output events are streamed (text delta/final, reasoning visibility policy, tool call lifecycle, status, run started/completed/errored, usage, diagnostics);
- whether the stream is runtime-wide, worker-scoped, or both;
- event envelope shape, event id/cursor semantics, ordering, backlog, reconnect behavior, and unknown/expired cursor handling;
- relationship between streamed output and transcript projection/event log persistence;
- Backend client/proxy expectations and how Browser receives the projection without connecting directly to Runtime;
- what is deliberately excluded from the stream, such as raw provider trace or raw full session log.


---

<!-- event: decision author: hare at: 2026-06-25T20:05:49Z -->

## Decision

Runtime WebSocket event stream は、新しい Worker output protocol を作らず `crates/protocol` の `protocol::Event` を Backend-facing observation payload として流す方針にする。Runtime WS は `protocol::Event` の variant allowlist / subset を定義せず、worker-scoped envelope に event id / cursor / worker id を付けて Worker event bus の `protocol::Event` を forward する。

Browser / Web UI は Runtime WS に直接接続しない。Backend が Runtime WS client になり、Browser-facing stream は Backend-owned projection layer を通す。この projection layer を、後続の Web 権限制御で observation allow/deny、thinking/tool output/diagnostic redaction、operation-capable command API forwarding allow/deny を差し込む境界にする。STJT では full auth model は実装せず、この seam を型と責務として作る。


---

<!-- event: intake_summary author: hare at: 2026-06-25T20:10:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T20:10:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T20:21:27Z from: ready to: planning reason: websocket_transport_decision_missing field: state -->

## State changed

ユーザー指示により planning に戻す。

Missing decision / information:
- WebSocket / event-stream transport は `00001KVZKSTJT` 自体で決定すべき設計点であり、未決定のまま ready/queue 対象として扱うのは不適切。
- 少なくとも、Backend-owned WebSocket client 方式を v0 で採用するか、SSE / polling / Backend proxy projection との責務分離をどう置くか、cursor/backlog/error semantics をどこまで固定するかを planning で再確認する必要がある。

Context checked:
- Ticket body: `worker-runtimeにWebSocket event stream serverを追加する` は `ws-server` feature、WebSocket observation endpoint、cursor resume、unknown/expired cursor diagnostics を実装対象としている。
- Relations: `00001KVZBCQH4` と `00001KVZKSTE2` に depends_on、`00001KVZSGT14` が本 Ticket に depends_on。
- Current state: 本 Ticket は queued ではなく `ready` だったが、WS を扱う予定の Ticket として routing/queue 前に設計判断へ戻す。

Why implementation latitude is insufficient:
- Transport choice / ownership boundary / Browser direct access exclusion / Backend proxy shape は local implementation tactic ではなく、後続 Backend/remote Runtime/Web Console の設計前提になる binding decision。

Next planning question/action:
- `worker-runtime` observation transport は v0 で WebSocket を採用するのか、それとも SSE/polling/Backend projection を優先するのか。
- WebSocket を採用する場合、Backend-owned client、cursor/backlog/unknown cursor、worker-scoped filtering、Browser-facing protocol non-goal の境界を明文化する。
- 後続 `00001KVZSGT14` など remote observation 依存 Ticket は、この判断後に readiness/relations を再確認する。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T20:24:40Z from: ready to: planning reason: cli_state field: state -->

## State changed

State changed to `planning`.


---

<!-- event: decision author: hare at: 2026-06-25T20:25:52Z -->

## Decision

STJT の主目的を Runtime WS server 単体から `Runtime -> Backend -> Client` の WebSocket observation proxy に広げる。Remote Runtime から Backend Runtime WS client が `protocol::Event` を受け取り、Backend-owned client-facing WS で `runtime_id + worker_id` keyed stream として流すところまでをこの Ticket の実装対象に含める。

Backend proxy は v0 では pass-through projection を基本にし、`protocol::Event` を別 output model へ変換しない。一方で Browser / future TUI は Runtime WS に直接接続せず、Backend-owned cursor/envelope/diagnostic を持つ stream だけを見る。Runtime endpoint / credential / socket path / session path は Client-facing authority に出さない。

権限系はこの Ticket で full auth / permission / redaction rule を実装しない。Backend proxy/projection layer に、後続で observe allow/deny、thinking/tool output/diagnostic redaction、action affordance、command API forwarding allow/deny を差し込める seam を作るに留める。

---

<!-- event: intake_summary author: hare at: 2026-06-25T20:30:38Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T20:30:38Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T20:34:20Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T20:36:24Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / workspace state を確認した。
- 本 Ticket は `00001KVZKSTE2` REST command server に depends_on。`00001KVZKSTE2` は本 routing pass で accepted され `inprogress` になった。
- WS observation proxy は Runtime process server surface と Backend proxy/client-facing stream を扱うため、REST command API/process wrapper の形が確定してから開始する。

Evidence checked:
- Ticket body: `Runtime -> Backend -> Client` WebSocket observation proxy、Runtime worker-scoped WS、Backend Runtime WS client、Client-facing WS、cursor/backlog/permission seam。
- Relations: outgoing `depends_on -> 00001KVZKSTE2`; incoming dependent Tickets include Web Console MVP, remote Runtime process, TUI migration。
- Orchestration plan: blocker record `orch-plan-20260625-203613-1` を追加。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZKSTE2` が review/merge/validation/done になった後に再 routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-26T04:22:18Z -->

## Decision

Routing decision: implementation_ready

Reason:
- `00001KVZKSTE2` REST command server は done。前回 blocker（REST process wrapper/API surface 未確定）は解消済み。
- 本 Ticket はユーザー指摘後に `Runtime/Backend WebSocket observation proxy` として設計判断を反映し再queuedされた。Ticket thread には、Runtime WS は `protocol::Event` を observation payload として流し、Browser/future TUI は Runtime へ直接接続せず Backend-owned projection/proxy を見る、という binding decision が記録済み。
- queued/inprogress 再確認時点で inprogress は 0 件。後続 remote/TUI/Web Console Tickets は本 Ticket に依存しているため、本 Ticket を次に受理する。

Evidence checked:
- Ticket body: `Runtime -> Backend -> Client` WebSocket observation proxy、Runtime worker-scoped WS、Backend Runtime WS client、Client-facing WS、cursor/backlog/permission seam、Non-goals。
- Thread decisions: `protocol::Event` を payload authority とする、Runtime WS は command/mutation tunnel にしない、Backend projection/proxy seam を作る、full auth/redaction policy は後続。
- Relations: outgoing dependencies `00001KVZBCQH4` core と `00001KVZKSTE2` REST server は done。incoming remote/Web Console/TUI Tickets は後続。
- Orchestration plan: accepted plan `orch-plan-20260626-042150-2` を記録。
- Workspace state: orchestration worktree clean; no inprogress Ticket.

IntentPacket:

Intent:
- Runtime process の worker-scoped WebSocket observation stream と、Backend-owned client-facing WebSocket proxy boundary を実装する。

Binding decisions / invariants:
- Runtime WS は Backend-facing internal observation API。Browser/future TUI は Runtime WS に直接接続しない。
- Payload authority は `crates/protocol` の `protocol::Event`。Runtime WS 独自の parallel output model や variant allowlist/subset を作らない。
- Runtime WS は command/mutation/user input を受け付けず、`protocol::Method` tunnel を作らない。
- Backend client-facing WS は Backend-owned opaque cursor/envelope/diagnostic を持ち、Runtime endpoint/credential/socket/session path を Client に露出しない。
- v0 は worker-scoped stream。runtime-wide stream、full auth/permission/redaction policy、Web Console UI、TUI migration、remote process lifecycle/discovery は Non-goals。
- REST command semantics は既存 `http-server` implementation に委譲し、この Ticket で再実装しない。

Requirements / acceptance criteria:
- `worker-runtime` に optional `ws-server` feature がある。
- Feature disabled でも core compile が通る。
- Runtime process exposes `GET /v1/workers/{worker_id}/events/ws?cursor=...` style worker-scoped observation endpoint。
- Runtime WS envelope includes Runtime-local opaque cursor/event id, worker id, and `protocol::Event` payload。
- Connect sends initial `protocol::Event::Snapshot` projection, then forwards Worker event bus `protocol::Event` payloads。
- Backend Runtime WS client consumes Runtime envelope and preserves `runtime_id + worker_id + runtime_cursor + protocol::Event` internally。
- Backend exposes Client-facing worker observation WS keyed by `runtime_id + worker_id` with Backend-local opaque cursor/envelope。
- Unknown/expired cursor, worker not found, runtime unavailable, upstream disconnect, malformed frame are typed diagnostics/errors。
- Tests cover Runtime WS, Backend upstream client/proxy delivery, cursor resume/duplicate-safe IDs, diagnostics, and worker-scoped filtering.

Implementation latitude:
- Exact Rust module split, WebSocket dependency, envelope structs, test fixtures, and Backend route shape may follow existing workspace-server/worker-runtime style。
- Bounded backlog implementation can be in-memory v0, as long as cursor semantics and diagnostics are explicit.
- Permission seam can be pass-through default with types/hooks for later policy.

Escalate if:
- Implementing this requires full auth/redaction policy, Web Console UI, TUI migration, remote process lifecycle/discovery, or Runtime command channel changes。
- `protocol::Event` cannot be serialized/forwarded without changing protocol crate public semantics。
- Backend Registry/Runtime handle shape from previous Tickets is insufficient and would require a broad redesign.

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Accidentally creating a command/mutation tunnel over WS。
- Leaking Runtime endpoint/token/socket/session path to Client-facing WS。
- Creating a second output/event model instead of forwarding `protocol::Event` payload。
- Feature gating/dependency leakage。
- Cursor/backlog/diagnostic semantics being too vague for remote/TUI/Web Console follow-ups。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T04:22:27Z from: queued to: inprogress reason: routing_accepted_after_rest_server_done field: state -->

## State changed

Routing decision: implementation_ready。

Dependency `00001KVZKSTE2` REST command server は done。Ticket body/thread、relations、workspace state、accepted plan を確認し、Runtime/Backend WebSocket observation proxy は unblocked と判断した。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T04:23:22Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KVZKSTJT-websocket-observation-proxy`
- Branch: `work/00001KVZKSTJT-websocket-observation-proxy`
- Coder Worker spawned: `yoi-coder-00001KVZKSTJT-ws-proxy`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/worker-runtime/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix`.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Next action:
- Wait for Coder implementation report, then route to reviewer.

---

<!-- event: implementation_report author: yoi-coder-00001KVZKSTJT-ws-proxy at: 2026-06-26T04:56:04Z -->

## Implementation report

Implementation report:

実装完了。Ticket ファイル / project records は変更していない。merge / close / cleanup 前で停止。

Commit:
- `9807accaf08f4322763dd7cd3536baf3dab8d10e feat: add worker observation websocket proxy`

変更概要:

### worker-runtime

変更ファイル:
- `crates/worker-runtime/Cargo.toml`
- `crates/worker-runtime/src/observation.rs`
- `crates/worker-runtime/src/runtime.rs`
- `crates/worker-runtime/src/http_server.rs`

実装内容:
- optional feature `ws-server` を追加。
  - `http-server` に加えて `axum/ws`, `protocol`, `futures`, `tokio/sync` を使用。
  - feature 無効時の core compile も確認済み。
- Runtime 側に Worker-scoped observation backlog / bus を追加。
  - Runtime-local opaque cursor: `wo_<hex sequence>`
  - bounded in-memory backlog v0
  - `observe_worker_event(worker_ref, protocol::Event)` で `protocol::Event` payload をそのまま observation bus に投入。
- Runtime HTTP server に worker-scoped WS endpoint を追加。
  - `GET /v1/workers/{worker_id}/events/ws?cursor=...`
- Runtime WS frame:
  - `RuntimeWorkerEventWsFrame::Event { envelope }`
  - `RuntimeWorkerEventWsFrame::Diagnostic { diagnostic }`
- Runtime WS envelope:
  - `cursor`
  - `event_id`
  - `worker_id`
  - `payload: protocol::Event`
- 接続時に `protocol::Event::Snapshot` を初期 projection として送信。
- live update は observation bus の `protocol::Event` payload を転送。
- client からの text/binary 等の入力は `runtime.observation_only` diagnostic で終了。
- malformed cursor / unknown-or-expired cursor / worker not found / bus closed / lagged backlog などを typed diagnostic/error 化。

### workspace-server

変更ファイル:
- `crates/workspace-server/Cargo.toml`
- `crates/workspace-server/src/lib.rs`
- `crates/workspace-server/src/observation.rs`
- `crates/workspace-server/src/server.rs`

実装内容:
- Backend-owned observation proxy module を追加。
- Runtime WS client を追加。
  - Runtime WS envelope を decode。
  - 内部表現として `runtime_id + worker_id + runtime_cursor + protocol::Event` を保持。
- Backend client-facing WS endpoint を追加。
  - `GET /api/runtimes/{runtime_id}/workers/{worker_id}/events/ws?cursor=...`
- Backend-local cursor/envelope を追加。
  - Backend cursor: `bo_<hex sequence>`
  - Runtime endpoint / credential / socket / session path は client-facing payload に出さない。
- Backend client-facing frame:
  - `ClientWorkerEventWsFrame::Event { envelope }`
  - `ClientWorkerEventWsFrame::Diagnostic { diagnostic }`
- Backend client-facing envelope:
  - `cursor`
  - `event_id`
  - `runtime_id`
  - `worker_id`
  - `payload: protocol::Event`
- Backend source config は server 内部設定 `runtime_event_sources` として保持。
- unknown worker/runtime source, malformed/unknown cursor, upstream disconnect, malformed upstream frame, observation-only client input を typed diagnostic/error 化。

### workspace/root/package

変更ファイル:
- `Cargo.toml`
- `Cargo.lock`
- `package.nix`

内容:
- `tokio-tungstenite = "0.29"` を workspace dependency に追加。
- `package.nix` の `cargoHash` を更新。

Validation:
- `cargo fmt --all`: success
- `cargo check -p worker-runtime`: success
- `cargo test -p worker-runtime --features ws-server`: success（14 lib tests + 3 bin tests + doctests）
- `cargo test -p yoi-workspace-server`: success（22 lib tests + bin/doctests）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `git diff --check --cached`: success
- `nix build .#yoi --no-link`: success

Review focus / residual risks:
- v0 backlog は in-memory bounded。process restart / memory eviction 後の cursor は unknown/expired 扱い。
- Runtime Snapshot projection は minimal projection。`protocol::Event::Snapshot` を使うが、entries / in-flight は現 Runtime adapter の範囲で空 projection。
- Backend source discovery は `runtime_event_sources` 内部 config seam。remote lifecycle/discovery や full registry redesign は Non-goals として未実装。
- Auth/redaction/policy は full 実装せず、後続拡張しやすい typed source/diagnostic/proxy boundary に留めた。
- Browser / future TUI には Backend endpoint のみを公開し、Runtime endpoint / credential / socket / session path は envelope に含めていない。

---
