<!-- event: create author: "yoi ticket" at: 2026-06-25T16:23:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:30:32Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:30:32Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T20:34:35Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T20:36:34Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / workspace state を確認した。
- 本 Ticket は WebSocket observation proxy `00001KVZKSTJT` と REST command server `00001KVZKSTE2` を前提にする remote worker-runtime process connection。
- `00001KVZKSTE2` は現在 inprogress、`00001KVZKSTJT` は queued/blocked。remote process connection を先に始めると transport/API shape を先取りして固定するため開始しない。

Evidence checked:
- Ticket body: remote Runtime process接続、Backend RuntimeRegistry source、REST/WebSocket client boundary、Non-goals。
- Relations: outgoing dependencies include `00001KVZKSTE2` / `00001KVZKSTJT` / `00001KVZKSV6C` 等。
- Orchestration plan: blocker record `orch-plan-20260625-203613-1` を追加。

Next action:
- 本 Ticket は queued のまま待機。
- REST command server と WebSocket observation proxy が done になった後に再 routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-26T05:48:59Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dependencies are done: `00001KVZKST83` FS store、`00001KVZKSTE2` REST command server、`00001KVZKSTJT` WebSocket observation proxy、`00001KVZKSV6C` RuntimeRegistry foundation。
- Existing relation to `00001KVZQHPNY` is `related`, not blocking, and says v0 can use builtin/default fallback where applicable.
- 本 Ticket は remote Runtime process connection / routing を主目的とし、Dynamic registration / full auth / Web Console / Profile config sync は Non-goals。
- 現在 `inprogress` は 0 件。remote Runtime connection は TUI migration の blocker なので優先して受理する。

Evidence checked:
- Ticket body: remote Runtime client handle、REST command API client、Runtime event stream client/proxy、config/policy boundary、Non-goals、acceptance criteria。
- Relations: outgoing dependencies to FS/REST/WS/Registry foundation are done。Incoming TUI dependency is downstream。
- Orchestration plan: accepted plan `orch-plan-20260626-054840-2` を記録。
- Workspace state: orchestration worktree clean、embedded Runtime registry Ticket done/cleanup 済み。

IntentPacket:

Intent:
- Workspace Backend `RuntimeRegistry` に remote worker-runtime process handle を追加し、Backend-owned REST/WS client で remote Runtime operations を route/proxy できるようにする。

Binding decisions / invariants:
- Browser は remote Runtime base URL / token / direct endpoint を知らない。
- Browser-facing authority は embedded/local と同じ `runtime_id + worker_id`。
- Remote command は worker-runtime REST command API に対する Backend-owned client。
- Remote observation は worker-runtime WS observation API に対する Backend-owned client/proxy。
- Dynamic registration、full auth/permission model、Backend Web Console、Profile/config bundle sync は実装しない。
- Embedded/local compatibility source の behavior を壊さず、remote source を implementation kind/diagnostics で区別する。

Requirements / acceptance criteria:
- Config-like data から remote Runtime client handle を登録できる。
- Runtime summary/status/capabilities、worker list/detail/create/input/stop/cancel/transcript/event proxy を route できる。
- Network/auth/timeout/remote unsupported/remote worker not found を typed Backend errors/diagnostics に map する。
- Browser-facing API/WS envelope に remote base URL/token/direct endpoint/socket/session path を露出しない。
- Focused workspace-server tests cover mocked remote routing and error mapping。

Implementation latitude:
- HTTP/WS client dependencies、mock remote test harness、internal source/handle trait shape、error type naming は Coder が既存 `workspace-server` pattern に合わせて選べる。
- v0 config-like data は in-memory/static constructor/test fixture でよい。Dynamic registration は不要。

Escalate if:
- worker-runtime REST/WS public API 変更が必要になる。
- Profile/config sync がないと remote Worker creation が成立しない場合。
- Browser-facing API schema を大きく変える必要がある場合。

Validation:
- `cargo fmt --all`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- remote endpoint/token/direct URL leak。
- worker_id-only routing regression。
- remote REST/WS client errors being collapsed into generic unavailable。
- scope creep into dynamic registration/auth/Web Console/Profile sync。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T05:49:13Z from: queued to: inprogress reason: routing_accepted_after_remote_runtime_prerequisites_done field: state -->

## State changed

Routing decision: implementation_ready。

FS store、REST command server、WebSocket observation proxy、RuntimeRegistry foundation は done。Ticket body / relations / workspace state / accepted plan を確認し、remote worker-runtime process connection slice は unblocked と判断した。Profile/config sync は related であり v0 builtin/default fallback が許容されているため blocker ではない。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T05:50:28Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KVZSGT14-remote-runtime-registry`
- Branch: `work/00001KVZSGT14-remote-runtime-registry`
- Coder Worker spawned: `yoi-coder-00001KVZSGT14-remote-runtime`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Queued capacity notes:
- `00001KVZQHPNY` remains queued to avoid worker creation/config routing churn while remote Runtime source is inprogress.
- `00001KVZ9JGK0` remains queued to avoid overlapping Backend/Web Console API surface changes while remote source routing stabilizes.
- `00001KW04A8K6` remains queued because it depends on this remote Runtime source work.

---
