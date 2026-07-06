<!-- event: create author: "yoi ticket" at: 2026-07-06T18:00:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-06T18:26:52Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-06T18:26:52Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-06T18:28:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-06T18:29:42Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は local Git worktree Execution Workspace materializer の v0 範囲、非目標、acceptance criteria が具体化されている。
- Objective `00001KWW44EXK` の境界（Runtime root / Execution Workspace / RepositoryPoint / materialization strategy / cleanup registry / dirty state policy）と整合している。
- `remote URI`, clone/fetch/cache, credential resolution, CoW/sandbox 強化、dirty snapshot/apply は非目標として明確。
- typed relation blocker は 0 件、OrchestrationPlan record は 0 件。
- queued notification は human authorized routing であり、今回の routing acceptance 後にのみ implementation side effect へ進める。

Evidence checked:
- Ticket body / thread / artifacts。
- Objective `00001KWW44EXK` body（Runtime execution workspace materialization direction, Runtime root separation, detached worktree strategy, clean point policy, cleanup registry）。
- `TicketRelationQuery(00001KWW9DYH4)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KWW9DYH4)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。inprogress は 0 件。
- visible Pods: prior repository-registry child Pods が idle で残っているが、`StopPod` 不使用方針のため停止せず、新規 unique Pod を使う。capacity blocker ではない。
- `TicketDoctor`: 0 errors / 既存 diagnostics のみ。
- Bounded code map: `crates/worker-runtime/src/{runtime.rs,worker_backend.rs,execution.rs,fs_store.rs,http_server.rs,management.rs,config_bundle.rs}`, `crates/workspace-server/src/{repositories.rs,server.rs,config.rs}`, `crates/protocol/src/lib.rs`。

IntentPacket:

Intent:
- Runtime Worker spawn の直前に Execution Workspace materialization boundary を追加し、configured local Git repository から Worker ごとの detached worktree を Runtime root 配下に作る v0 materializer を実装する。
- Worker の workspace scope / cwd を source repository root ではなく materialized worktree path にすることで、同じ local Git repository を対象に複数 Worker を spawn しても source root scope allocation conflict で失敗しないようにする。

Binding decisions / invariants:
- Execution Workspace は `.yoi` / Backend workspace store ではなく Runtime root 配下に作る。
- v0 は local Git repository URI のみ対応する。remote URI / non-Git provider / clone/fetch/cache / credentials は typed diagnostic で拒否または非対応扱い。
- worktree は branch 名ではなく resolved commit に対する detached worktree として作る。v0 materialization では Worker 用 branch を作らない。
- dirty local changes は暗黙に含めない。v0 は `clean_point_only` とし、dirty state は typed diagnostic / test evidence で明示する。
- created allocation には repository id、resolved commit/tree など evidence、materializer kind、cleanup target、status/cleanup policy に相当する record を持たせる。
- Worker stop / cleanup 時に worktree cleanup できる方針または最低限の cleanup record を持つ。
- Browser/API-facing diagnostics は raw host path を不必要に漏らさない。
- `RepositoryId` / `RepositorySelector` / `RepositoryPoint` の将来境界を壊さず、Git branch/tag/hash 固定の抽象にしない。

Requirements / acceptance criteria:
- `CreateWorkerRequest` または Runtime-side spawn path に Execution Workspace materialization request / allocation 境界が追加されている。
- local Git repository `uri = "."` 相当から resolved commit を取得し、Runtime root 配下の `execution-workspaces/<allocation-id>/root/<repository-id>` に `git worktree add --detach` できる。
- Worker spawn 時の scope/cwd は source repository root ではなく materialized worktree を指す。
- 同じ local Git repository を対象に複数 Worker を spawn しても source root scope conflict で失敗しない。
- unsupported non-local URI / non-Git provider は typed diagnostic。
- dirty local workspace を暗黙に含めず clean-point-only policy であることが tests で分かる。

Implementation latitude:
- Runtime-side 型名、module 分割、allocation record の永続化形式、cleanup status 名は既存 `worker-runtime` style に合わせてよい。
- v0 で Runtime repository cache/bare mirror を作らず、configured local repository 自体を source cache 相当として扱ってよい。
- cleanup は full lifecycle 完成ではなく、最低限の record と stop/cleanup hook を優先してよい。ただし orphan worktree cleanup の方向が明示されていること。
- Browser/API response は detailed host paths を避け、debug/internal logs/tests では必要に応じて raw path を扱ってよい。

Escalate if:
- remote clone/fetch/cache, credentials, dirty snapshot/apply, synthetic branch/export policy, strong sandbox/container/network policy, multi-repository mount, or Backend store migration が必要になる場合。
- Runtime / Worker launch protocol の public boundary を広げ、raw source path/secret/internal runtime path を Browser-facing API に出す必要がある場合。
- RepositoryPoint / RepositorySelector の semantic boundary を変更する必要がある場合。

Validation:
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更がある場合だけ実行し、未実行なら理由を報告する。

Current code map:
- Runtime worker spawn/control: `crates/worker-runtime/src/runtime.rs`, `worker_backend.rs`, `execution.rs`, `http_server.rs`, `management.rs`。
- Runtime persistence/root: `crates/worker-runtime/src/fs_store.rs`, `catalog.rs`, `config_bundle.rs`。
- Repository registry / point context: `crates/workspace-server/src/repositories.rs`, `config.rs`, `server.rs`。
- Protocol/request shapes: `crates/protocol/src/lib.rs` and worker-runtime request/response structs。

Critical risks / reviewer focus:
- No source repository root scope/cwd is passed to Worker as the normal path after materialization。
- detached worktree is created at resolved commit, not branch checkout。
- dirty source repository state is rejected or clearly excluded by clean_point_only tests。
- allocation record/evidence/cleanup target is present and not only an ephemeral path local variable。
- unsupported providers/remotes fail closed with typed sanitized diagnostics。
- materializer boundary remains strategy-extensible and does not hard-code Git worktree as the only future model。

---

<!-- event: state_changed author: orchestrator at: 2026-07-06T18:30:00Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- Objective `00001KWW44EXK` の Execution Workspace materialization strategy / dirty policy / Runtime root separation / cleanup registry context。
- `TicketRelationQuery(00001KWW9DYH4)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWW9DYH4)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。
- visible Pods / TicketDoctor / bounded code map を確認。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket の scope は local Git detached worktree materializer v0 に閉じており、remote clone/fetch/cache, credentials, dirty snapshot/apply, strong sandbox 等は非目標として明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-06T18:31:11Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KWW9DYH4-execution-workspace-materializer`
  - Branch: `work/00001KWW9DYH4-execution-workspace-materializer`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KWW9DYH4-exec-workspace` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-06T18:57:42Z -->

## Implementation report

Implementation progress report:

- Coder Pod completed implementation and committed `8b7a5da0 feat: materialize execution workspaces` on branch `work/00001KWW9DYH4-execution-workspace-materializer`.
- Worktree status was clean after commit.
- Changed files include:
  - `crates/worker-runtime/src/execution_workspace.rs` (new)
  - `crates/worker-runtime/src/{catalog.rs,execution.rs,http_server.rs,lib.rs,main.rs,runtime.rs,worker_backend.rs}`
  - `crates/workspace-server/src/{companion.rs,hosts.rs,server.rs}`
- Implementation summary:
  - Runtime-side Execution Workspace request/status types and local Git worktree materializer。
  - detached Git worktree creation under Runtime root: `execution-workspaces/<allocation-id>/root/<repository-id>`。
  - clean HEAD/selector commit and tree evidence resolution。
  - dirty source rejection under `clean_point_only`。
  - typed diagnostics for remote URI / non-Git provider / unsupported policy。
  - materialization/cleanup evidence persistence and best-effort worktree cleanup on Worker stop。
  - Worker spawn cwd/scope uses materialized worktree path, not source repository root。
  - workspace-server embedded runtime startup and Worker spawn pass configured repositories into materialization request。
  - API-facing execution workspace status summaries remain sanitized/path-free。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（34 lib tests + 5 main tests + doc tests）
  - `cargo test -p yoi-workspace-server`: pass（63 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（15 tests）
  - `yoi ticket doctor`: ok
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。
- External review will be requested via a read-only sibling Reviewer Pod. `StopPod` は使わない。

---

<!-- event: review author: reviewer at: 2026-07-06T19:06:24Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blocker:
- 実装が Browser/API-facing spawn boundary を広げ、API caller から full `ExecutionWorkspaceRequest`（`ExecutionWorkspaceRepository.local_path: Option<PathBuf>` を含む）を受け取れる形になっている。
  - `crates/workspace-server/src/hosts.rs:263-283` は `WorkerSpawnRequest` を browser-safe として raw workspace roots/storage paths を受け取らない契約にしているが、現在は `execution_workspace: Option<ExecutionWorkspaceRequest>` を含む。
  - `crates/worker-runtime/src/catalog.rs:58-66` の request は raw `local_path` を運べる。
  - `/api/runtimes/{runtime_id}/workers` はこの request shape を受け取り、embedded/remote host spawn path が `CreateWorkerRequest` に pass-through している。
- これは Ticket / routing invariant の「raw source/internal paths を含む public launch boundary 拡張を避ける。必要なら escalate」に反し、既存コード上の browser-safe contract とも衝突している。
- safer `/api/workers` path は server-side repository config から `repository_id` により解決しているため、generic API も caller-supplied raw `local_path` authority を受け取らない形にするか、Execution Workspace materialization request の組み立てを host/runtime 内部境界に閉じる必要がある。

Evidence supporting the rest of the implementation:
- Runtime/backend materialization boundary は存在する。`Runtime::create_worker` が request を backend に渡し、`WorkerRuntimeExecutionBackend::spawn_worker` が spawn 前に materializer を呼ぶ。
- local materializer は `execution-workspaces/<allocation-id>/root/<repository-id>` 配下に resolved commit の detached worktree を作成し、branch checkout はしない。
- Worker `workspace_root` / `cwd` は `Worker::from_manifest_with_context` 前に materialized binding に差し替えられている。
- dirty source は `clean_point_only` で拒否され、remote/non-Git は typed diagnostic code を返す。
- allocation evidence/status/cleanup target/policy は記録・永続化され、worker stop cleanup は worktree removal / cleanup-pending 更新を行う。
- tests は detached materialization、distinct paths、dirty rejection、unsupported diagnostics、cleanup、spawn cwd substitution を cover している。

Validation reviewed:
- Ticket / Objective と implementation diff/files を implementation worktree 内で read-only review。
- Orchestrator reported validation pass（`git diff --check`, worker-runtime/workspace-server tests, `cargo check -p yoi`, web checks/tests, `yoi ticket doctor`）を確認。

Non-blocking follow-ups:
- `Runtime::stop_runtime` は currently workers を stopped に mark するだけなので、per-worker stop 以外の runtime-stop/orphan-sweeper cleanup を後続で検討するとよい。
- external HTTP/API error body で materializer diagnostic code を message string だけでなく structured に保つ余地がある。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-06T19:17:17Z -->

## Implementation report

Review-fix implementation report:

- Coder Pod completed requested changes and committed follow-up `c90ebf24 fix: keep execution workspace paths internal` on branch `work/00001KWW9DYH4-execution-workspace-materializer`.
- Worktree status was clean after commit.
- Fix summary:
  - Browser/API-facing `WorkerSpawnRequest` no longer deserializes full `ExecutionWorkspaceRequest` or caller-supplied raw `local_path`.
  - Added browser-safe execution workspace request shape with `repository_id` and optional `selector` only。
  - Added `deny_unknown_fields` so raw fields such as `execution_workspace.local_path` and unexpected fields are rejected at the API boundary。
  - Server-side host code resolves configured repository registry into internal `ExecutionWorkspaceRequest`; raw `local_path` construction stays behind trusted host/server boundary。
  - Existing `/api/workers` repository-id safe behavior remains preserved。
  - Added focused API tests for rejecting raw `execution_workspace.local_path` and accepting safe repository-id/selector materialization without exposing raw workspace path。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（34 lib tests + 5 main tests + doc tests）
  - `cargo test -p yoi-workspace-server`: pass（65 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（15 tests）
  - `yoi ticket doctor`: ok
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。
- Requesting follow-up external review against `8b7a5da0..c90ebf24` and full combined implementation。

---
