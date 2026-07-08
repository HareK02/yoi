---
title: 'Route Ticket and Objective access through Backend APIs'
state: 'planning'
created_at: '2026-07-08T12:26:33Z'
updated_at: '2026-07-08T12:27:44Z'
assignee: null
---

## 背景

Ticket / Objective は Workspace product state であり、Backend が authority を持つ。Runtime / Worker / tools が `.yoi/tickets` や `.yoi/objectives` を直接読む構造を続けると、Workspace filesystem authority が Runtime 側へ漏れ、Browser API / Runtime API / tool 実装の境界が崩れる。

今後は Ticket / Objective の read/query/update は Backend API 経由に寄せる。Backend は既存の typed Ticket/Objective backend を内部実装として使ってよいが、Runtime/Worker/tools からは raw filesystem path や local backend root を見せない。

Memory / Workflow も同じ方向だが、Memory は抽出・consolidation・正本ストア・更新頻度の問題があり、Workflow は active workflow state / compaction / runtime state の問題があるため、本 Ticket では扱わず別途設計・実装 Ticket に分離する。

## 実装順序

- depends_on: `00001KX0G06VA` — Runtime -> Backend resource fetch REST API / resource handle 境界を先に使えるようにする。
- 本 Ticket: Ticket / Objective の Backend API と tool 経路の切り替えを実装する。
- 後続: Memory / Knowledge access と Workflow state access は、抽出・正本・更新モデルを整理した別 Ticket で扱う。

## 実装目的

- Browser / Runtime / Worker tools は Ticket / Objective を Backend API 経由で扱う。
- Runtime は `.yoi/tickets` / `.yoi/objectives` / local backend root を直接読まない。
- Ticket / Objective tool 実装は Backend-owned API/resource capability を使う。
- Backend は Ticket / Objective の read/query/update authority、validation、audit、diagnostic を持つ。
- Browser-facing API と Runtime-internal API は同じ Backend service layer を使うが、authority / redaction / mutation affordance は分ける。

## 実装内容

### 1. Backend service layer を追加する

Workspace Backend 内に Ticket / Objective service layer を追加する。これは既存 typed backend を内包し、Browser-facing API と Runtime-internal resource/tool API の共通実装になる。

実装すること:

- Ticket list / show / thread / artifacts / relations / resolution read。
- Ticket comment / review / state transition / close mutation。
- Objective list / show / relation / update / comment 相当の read/mutation。
- state transition validation / lifecycle authority / optimistic update validation。
- typed diagnostics。
- audit event / correlation id。

### 2. Browser-facing Ticket / Objective API を追加・整理する

Workspace-scoped Browser API から Ticket / Objective を read/update できるようにする。

想定 endpoint:

- `GET /api/w/<workspace-id>/tickets`
- `GET /api/w/<workspace-id>/tickets/<ticket-id>`
- `POST /api/w/<workspace-id>/tickets/<ticket-id>/comments`
- `POST /api/w/<workspace-id>/tickets/<ticket-id>/reviews`
- `POST /api/w/<workspace-id>/tickets/<ticket-id>/state`
- `POST /api/w/<workspace-id>/tickets/<ticket-id>/close`
- `GET /api/w/<workspace-id>/objectives`
- `GET /api/w/<workspace-id>/objectives/<objective-id>`
- Objective update/comment endpoint は既存 Objective backend capability に合わせて追加する。

Browser-facing response に出さないもの:

- host absolute path。
- local backend root。
- runtime endpoint / resource handle / credential。
- session/socket/cache path。
- raw secret-like artifact contents。

### 3. Runtime-internal resource/tool API を追加する

Runtime / Worker tools が Ticket / Objective を扱うための Runtime-internal resource kinds を追加する。

追加する resource kind:

- `ticket_list`
- `ticket_read`
- `ticket_comment`
- `ticket_review`
- `ticket_state_update`
- `ticket_close`
- `objective_list`
- `objective_read`
- `objective_update`
- `objective_comment`

実装すること:

- Backend-issued handle / capability を検証する。
- runtime id / worker id / workspace id / operation binding を検証する。
- read と mutation の authority を分ける。
- mutation は typed Backend service layer を通す。
- diagnostics を Worker tool result に返せる形にする。

### 4. Tool 実装を Backend API 経由へ切り替える

Worker/Runtime tool 実装が local filesystem backend を直接開かないようにする。

対象:

- Ticket list/show/comment/review/state/close 系 tool。
- Objective list/show/update/comment 系 tool。
- Orchestrator / Intake / Reviewer / Coder role tools が使う Ticket/Objective access path。

実装すること:

- tool context に Backend resource client / capability を渡す。
- tool は Backend API を呼ぶ。
- local backend direct path は Backend process 内または compatibility-only tests に隔離する。
- Runtime が Workspace filesystem authority を持たない状態でも tools が動くことを確認する。

### 5. Panel / Workspace UI の既存直接読みに注意する

Panel / Workspace Browser が local file projection を持つ場合も、今後の authority は Backend API に寄せる。v0 で完全移行できない場合は compatibility path として明示する。

実装すること:

- Browser Workspace UI は Backend API を使う。
- local file scan / heuristic projection が残る場合は read-only compatibility とし、mutation authority にはしない。
- Ticket lifecycle authority は typed Backend service layer に集約する。

## 受け入れ条件

- Workspace Backend に Ticket service layer と Objective service layer が実装されている。
- Browser-facing API で Ticket list/show/comment/review/state/close ができる。
- Browser-facing API で Objective list/show/update/comment 相当ができる。
- Runtime-internal resource kinds for Ticket / Objective が実装されている。
- Ticket / Objective tools は Runtime から local `.yoi` filesystem を読まず Backend API/resource handle 経由で動く。
- Runtime が local Ticket/Objective backend root を持たない構成で focused tests が通る。
- mutation は Backend service layer の lifecycle validation / optimistic update / audit を通る。
- Browser-facing response と Worker tool result に host absolute path、backend root、runtime endpoint、resource credential、secret-like artifact contents が露出しない。
- unauthorized / expired handle、wrong runtime/worker/workspace binding、invalid lifecycle transition、stale revision、missing record は typed diagnostic になる。
- Existing `yoi ticket` CLI / local typed backend は Backend internal implementation または compatibility surface として壊れない。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Memory / Knowledge API/tool migration。抽出・consolidation・正本・更新頻度を含めて別 Ticket で扱う。
- Workflow / active workflow state API/tool migration。compaction / runtime state / history との関係を別 Ticket で扱う。
- Multi-user auth / RBAC の完成。
- Ticket schema migration。
- Objective schema redesign。
- Browser full project-management UI の完成。
