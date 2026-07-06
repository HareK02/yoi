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
