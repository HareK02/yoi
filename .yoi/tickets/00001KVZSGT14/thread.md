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

<!-- event: implementation_report author: yoi-coder-00001KVZSGT14-remote-runtime at: 2026-06-26T06:16:32Z -->

## Implementation report

Implementation report:

実装完了。マージ・Ticket 更新・close・cleanup は行っていない。

Commit:
- `aeb12b3b8e16a948e6f8387f6fb50fec60c53cc4 feat: add remote runtime registry source`

変更ファイル:
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`
- `crates/workspace-server/src/lib.rs`
- `crates/workspace-server/Cargo.toml`
- `Cargo.lock`
- `package.nix`

実装概要 / API 変更:
- `RuntimeRegistry` に remote worker-runtime source を追加。
  - `RemoteRuntimeConfig`
  - `RemoteWorkerRuntime`
  - backend-owned REST client による remote Runtime 操作
  - backend-owned WS observation source の生成
- Browser-facing の authority は `runtime_id + worker_id` のまま維持。
  - remote base URL / token / direct endpoint は API payload に出さない設計。
  - remote config の `Debug` も base URL/token を redaction。
- remote Runtime source summary を embedded/local と区別。
  - `RuntimeSourceKind::RemoteHttp` を active source として利用。
  - diagnostics も remote backend proxy として区別。
- routing 対応:
  - runtime summary/status/capabilities
  - host list
  - worker list/detail
  - worker create
  - input
  - stop
  - cancel
  - transcript
  - WS event proxy source fallback
- Browser/API endpoint 追加:
  - `POST /api/runtimes/{runtime_id}/workers/{worker_id}/stop`
  - `POST /api/runtimes/{runtime_id}/workers/{worker_id}/cancel`
- error mapping:
  - remote auth failure -> `RuntimeOperationFailed` / HTTP 401
  - remote timeout -> HTTP 504
  - unsupported -> HTTP 501
  - other remote operation failures -> HTTP 502
  - remote worker not found remains typed worker lookup failure path
- tests:
  - mocked remote HTTP routing test
  - secret/base URL non-leak assertion
  - backend-owned WS observation source assertion
  - remote auth error typed mapping test

Validation:
- `cargo fmt --all`: success
- `cargo test -p yoi-workspace-server`: success（31 passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success after `package.nix` cargoHash update

Residual risks / review focus:
- remote HTTP client は `reqwest::blocking` を利用。現状の `RuntimeRegistry` trait が sync なので変更量を抑えているが、将来 remote 操作の並行性/latency を重視するなら async trait 化または server handler 側 blocking isolation を検討。
- v0 remote config は static/in-memory registration で、dynamic registration / full auth model / Profile config sync は非実装。
- remote Worker create は既存 embedded mapping と同じ `CreateWorkerRequest::tools_less` ベース。remote 側で Profile/config bundle sync が必要になる場合は別 Ticket 境界。
- WS observation は backend-private source fallback を追加。実 remote WS e2e は mock HTTP unit test ではなく既存 proxy tests + source生成テストでの確認に留めている。

---

<!-- event: review author: yoi-reviewer-00001KVZSGT14-remote-runtime at: 2026-06-26T06:21:31Z status: request_changes -->

## Review: request changes

Review result: request_changes

ブロッカー:

- `RuntimeObservationSourceConfig` が `#[derive(Debug)]` のまま `endpoint` と `bearer_token` を保持している（`crates/workspace-server/src/observation.rs:13-19`）。さらに `BackendObservationProxy` も `#[derive(Debug)]` でこの source map を内包している（`observation.rs:184-187`）。
- remote source ではここに実 remote WS endpoint と token が入る（`hosts.rs:1642-1647`）ため、`format!("{:?}", source/proxy)` や将来の debug log で base URL / direct endpoint / bearer token が漏れる。
- ユーザー指定の focus「API/WS payloads or Debug/log-worthy types で漏らさない」に反している。

確認できた良い点:
- Browser-facing REST API は概ね `runtime_id + worker_id` routing になっており、worker_id-only route は見当たらない。
- remote command は Backend-owned `reqwest::blocking` REST client、observation は Backend-owned WS proxy/client 境界で実装されている。
- API/WS の通常 payload 型には remote base URL/token/direct endpoint は載っていない。
- auth/timeout/unsupported/remote error/worker-not-found は typed diagnostic/error に寄せる実装になっている。
- dynamic registration / full auth / WebConsole / Profile sync への scope creep は見当たらない。
- `reqwest::blocking` は async handler 内で Tokio worker を最大 timeout 分ブロックするリスクがあるが、v0 の同期 `RuntimeRegistry` 境界かつ timeout ありなら今回は許容可能な設計負債と判断する。

必要な修正:
- `RuntimeObservationSourceConfig` と、それを内包する debug/log-worthy 型の `Debug` を手書き redaction にするか derive を外す。
- base URL / endpoint / token が `Debug` 出力に含まれない focused test を追加する。

検証: read-only で対象 commit/diff/source/tests を確認。テスト実行は未実施。

---

<!-- event: implementation_report author: yoi-coder-00001KVZSGT14-remote-runtime at: 2026-06-26T06:27:50Z -->

## Implementation report

Follow-up implementation report after review request_changes:

Commit:
- `38ff7d8f807256ec654fcf5655e04dcbad4d9041 fix: redact observation source debug output`

修正内容:
- `RuntimeObservationSourceConfig` の `Debug` derive を廃止し、手動 `Debug` 実装に変更。
  - `runtime_id` / `worker_id` は残す。
  - `endpoint` は `<backend-private>`。
  - `bearer_token` は `<redacted>`。
- `BackendObservationProxy` の `Debug` derive を廃止し、手動 `Debug` 実装に変更。
  - source map / state の中身は出さず、`source_count` と omitted state のみ表示。
- focused test を追加。
  - source config の Debug に remote endpoint/base URL/path/token が含まれないこと。
  - containing proxy の Debug に remote endpoint/base URL/path/token が含まれないこと。

Validation:
- `cargo fmt --all`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- 今回の修正対象だった Debug/log-worthy 経路の漏洩は塞いだ。
- `RuntimeObservationSourceConfig` 自体は Backend 内部で real endpoint/token を保持し続ける。これは remote WS proxy 実行に必要な内部保持であり、API/WS payload には出していない。
- 将来 `Display` や手動ログで field を直接出す実装を追加する場合は別途注意が必要。

---

<!-- event: review author: yoi-reviewer-00001KVZSGT14-remote-runtime at: 2026-06-26T06:29:12Z status: approve -->

## Review: approve

Review result: approve

ブロッカーは解消済み。

確認内容:
- `RuntimeObservationSourceConfig` は manual `Debug` になり、`runtime_id` / `worker_id` のみ実値、`endpoint` は `<backend-private>`、`bearer_token` は `<redacted>` 表示になっている。
- `BackendObservationProxy` も manual `Debug` になり、source map / state internals を出さず `source_count` と omitted marker だけを出すため、内包する remote endpoint/token が Debug 経由で漏れない。
- 追加テストは source config と proxy Debug の forbidden host/path/token 非混入を検証している。
- 差分は `observation.rs` の redaction/test に限定され、API/WS payload 形状や remote routing への scope creep は見当たらない。

検証:
- read-only inspection のみ実施。テスト実行はしていない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T06:29:53Z -->

## Implementation report

Integration / validation report:

Integration:
- Implementation branch `work/00001KVZSGT14-remote-runtime-registry` を orchestration branch に merge 済み。
- Merge commit: `bbb5d68c merge: 00001KVZSGT14 remote runtime registry`

Included implementation commits:
- `aeb12b3b8e16a948e6f8387f6fb50fec60c53cc4 feat: add remote runtime registry source`
- `38ff7d8f807256ec654fcf5655e04dcbad4d9041 fix: redact observation source debug output`

Validation in Orchestrator worktree:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success（33 tests passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Review:
- Reviewer approve 済み。前回 blocker だった Debug/log-worthy endpoint/token redaction は解消済み。

Outcome:
- Acceptance criteria を満たしたため `done` へ進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T06:30:04Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Backend RuntimeRegistry remote worker-runtime process connection の実装、review、merge、Orchestrator validation が完了した。

Done evidence:
- Merge commit: `bbb5d68c merge: 00001KVZSGT14 remote runtime registry`
- Reviewer approve 済み。
- Orchestrator validation:
  - `cargo fmt --all --check`: success
  - `cargo test -p yoi-workspace-server`: success（33 tests passed）
  - `cargo check -p yoi`: success
  - `git diff --check`: success
  - `nix build .#yoi --no-link`: success

Scope:
- Backend RuntimeRegistry に remote worker-runtime process source/handle を追加し、Backend-owned REST/WS client 経由で route/proxy できるようにした。
- Dynamic registration / full auth / Web Console / Profile config sync は Non-goals として未実装。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T06:30:50Z -->

## Implementation report

Cleanup report:

- Child Workers stopped one-by-one and scope reclaimed:
  - `yoi-coder-00001KVZSGT14-remote-runtime`
  - `yoi-reviewer-00001KVZSGT14-remote-runtime`
- Child implementation worktree removed:
  - `/home/hare/Projects/yoi/.worktree/00001KVZSGT14-remote-runtime-registry`
- Child implementation branch removed:
  - `work/00001KVZSGT14-remote-runtime-registry`

Operational note:
- StopPod was executed sequentially, not in parallel.

---

<!-- event: state_changed author: hare at: 2026-06-26T17:46:04Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-26T17:46:04Z status: closed -->

## 完了

Completed, reviewed, validated, and merged into develop.


---
