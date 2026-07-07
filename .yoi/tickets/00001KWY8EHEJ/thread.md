<!-- event: create author: "yoi ticket" at: 2026-07-07T12:22:06Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-07T12:26:36Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-07T12:26:36Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-07T13:40:23Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-07T13:41:22Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Browser-managed Execution Workspace の pre-allocate 方式、API/UI/spawn 接続、`relative_cwd` validation、path leak invariant、cleanup policy の v0 方針を具体化している。
- 先行 Runtime-side materializer / Repository registry / workspace-id scoped API が orchestration branch に統合済みで、Browser-facing API を `/api/w/<workspace-id>/...` に追加する実装基盤がある。
- raw host path / internal materialization request を Browser payload に露出しない binding invariant が明確で、Worker spawn は allocation id + safe `relative_cwd` で接続する要求になっている。
- typed relation blocker は 0 件、OrchestrationPlan record は 0 件。
- queued notification は human authorized routing であり、今回の routing acceptance 後にのみ implementation side effect へ進める。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWY8EHEJ)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KWY8EHEJ)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。inprogress は 0 件。
- visible Pods: previous child Pods が idle で残っているが、`StopPod` 不使用方針のため停止せず、新規 unique Pod を使う。capacity blocker ではない。
- `TicketDoctor`: 0 errors / 既存 diagnostics のみ。
- Bounded code map: `crates/worker-runtime/src/{catalog.rs,execution_workspace.rs,runtime.rs,worker_backend.rs,http_server.rs}`, `crates/workspace-server/src/{server.rs,hosts.rs,repositories.rs,config.rs}`, `web/workspace/src/lib/workspace-sidebar/**`, scoped routes/API under `web/workspace/src/routes/w/[workspaceId]/**`。

IntentPacket:

Intent:
- Browser-facing API から configured repository + selector + policy により Execution Workspace を事前作成し、Browser には allocation id と safe summary だけを返す。
- Worker launch UI/API で作成済み Execution Workspace を選択し、Worker spawn 時に allocation id と safe `relative_cwd` を Runtime materializer / Worker spawn boundary に接続する。
- Worker detail/list に execution workspace の safe summary を表示し、cleanup policy と手動 cleanup 操作の v0 方針を実装/明示する。

Binding decisions / invariants:
- Browser から raw host path、backend-private absolute path、raw `ExecutionWorkspaceRequest`、internal runtime path を送らせない/返さない。
- pre-allocate 方式を優先する。単発 spawn request だけで暗黙 materialize する経路は後回し。
- v0 materializer は existing local Git detached worktree 方針を使う。remote clone/cache、dirty source inclusion は扱わない。
- Worker spawn は作成済み allocation id と任意の `relative_cwd` を選ぶ。
- `relative_cwd` は materialized workspace root からの相対 path とし、absolute path、`..` escape、symlink escape、存在しない/不正 directory を拒否する。
- Browser-facing status/detail/list は safe summary のみ。内部 path / cleanup target raw path は露出しない。
- cleanup policy と manual cleanup operation は v0 の範囲で明示し、cleanup diagnostics は typed/sanitized にする。
- Workspace-scoped Browser API は `/api/w/<workspace-id>/...` を使い、Runtime `/v1/...` API を Browser contract として広げない。

Requirements / acceptance criteria:
- `POST /api/w/<workspace-id>/execution-workspaces` 相当で configured repository + selector から Execution Workspace を作成できる。
- `GET /api/w/<workspace-id>/execution-workspaces` 相当で safe summary list を取得できる。
- 必要なら detail / cleanup endpoint を scoped API に追加し、safe summary / typed diagnostic を返す。
- Worker launch UI で Execution Workspace を選択でき、raw path ではなく safe id/selector 情報を送る。
- Worker spawn 時に選択 allocation が Runtime の materialized workspace に接続され、Worker cwd/scope は materialized worktree 配下になる。
- invalid repository / dirty unsupported / remote unsupported / invalid `relative_cwd` は typed diagnostic。
- Worker detail/list に execution workspace safe summary を表示する。
- frontend check/test、workspace-server tests、`yoi ticket doctor`、`nix build .#yoi` を通す。

Implementation latitude:
- Allocation store の所在/形式、API endpoint 名、UI placement、cleanup status 名、test helper は既存 workspace-server / worker-runtime / web style に合わせてよい。
- v0 では Runtime materializer の既存 allocation evidence を再利用し、Browser-managed allocation registry は必要最小限でよい。
- Worker launch UI は既存 New Worker form に Execution Workspace select / create link / refresh を追加する形でよい。
- `relative_cwd` は empty/`.` を workspace root と扱ってよい。

Escalate if:
- raw host path や internal runtime path を Browser contract に出さないと満たせない場合。
- remote clone/cache, dirty snapshot/apply, credential resolution, multi-repository mount, strong sandbox policy, or cross-Runtime allocation migration が必要になる場合。
- pre-allocate 方式でなく implicit one-shot materialization を主経路にする必要が出た場合。

Validation:
- `cd web/workspace && deno task check && deno task test`
- `cargo test -p yoi-workspace-server`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`（Ticket acceptance に含まれるため実行する。実行不能なら理由を implementation report に明記する）

Current code map:
- Runtime Execution Workspace: `crates/worker-runtime/src/catalog.rs`, `execution_workspace.rs`, `runtime.rs`, `worker_backend.rs`, `http_server.rs`。
- Workspace Backend host/API: `crates/workspace-server/src/server.rs`, `hosts.rs`, `repositories.rs`, `config.rs`。
- Worker launch UI and API helpers: `web/workspace/src/lib/workspace-sidebar/WorkersNavSection.svelte`, worker launch model/tests, workspace API helper, scoped route layout。
- Worker detail/console: scoped worker console route and runtime worker detail/list projections。

Critical risks / reviewer focus:
- Browser payload/response must not expose raw path or raw internal materialization request。
- `allocation_id` must not become ambient authority across mismatched workspace/runtime/repository contexts。
- `relative_cwd` validation must handle absolute paths, `..`, symlink escape, missing/non-directory paths。
- cleanup operation must not delete outside materialized allocation root。
- Worker spawn should use existing materialized allocation rather than creating hidden implicit workspaces as the primary flow。
- Runtime `/v1` remains internal/trusted; Browser uses scoped workspace API。

---

<!-- event: state_changed author: orchestrator at: 2026-07-07T13:41:43Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWY8EHEJ)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWY8EHEJ)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。
- visible Pods / TicketDoctor / bounded code map を確認。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket の scope は Browser-managed pre-allocated Execution Workspace と Worker spawn 接続に閉じており、remote clone/cache, dirty source inclusion, raw path Browser contract, implicit one-shot materialization は非目標/後回しとして明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-07T13:42:50Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KWY8EHEJ-browser-execution-workspaces`
  - Branch: `work/00001KWY8EHEJ-browser-execution-workspaces`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KWY8EHEJ-browser-execws` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---
