<!-- event: create author: "yoi ticket" at: 2026-06-24T12:29:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T13:20:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T13:20:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T19:04:55Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T19:06:45Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は Backend internal Orchestrator runtime / Kanban operation / durable orchestration event / WorkerRuntime registry identity / spawn intent / session overview boundary / safety authority を具体的に列挙している。
- `TicketRelationQuery` は blocking relation 0 件、routing 前 `TicketOrchestrationPlanQuery` は既存 plan 0 件だった。
- Ticket の受け入れ条件は「planning artifact を明文化する」性質であり、Backend internal Orchestrator の full implementation は non-goal と明記されている。したがって Coder に実装詳細を固定させるのではなく、maintained design/planning document と必要最小限の boundary alignment を作る task として bounded に進められる。
- risk flags 相当の domain は orchestration authority / runtime identity / durable event / raw session boundary だが、Bash/raw filesystem/raw socket/raw session ingest を authority にしない、`queued -> inprogress` acceptance を守る、Browser が raw local path を渡さない、という invariant が明示されている。risk は reviewer focus と escalation condition に反映すればよく、planning return 理由にはならない。
- 現在 inprogress Ticket は 0 件、orchestration worktree は clean。既存 worktree は unrelated paused Ctrl-X cancel worktree のみで、この Ticket の branch/worktree はまだ無い。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: relation 0 件、routing 前 plan 0 件。accepted plan `orch-plan-20260624-190611-1` を記録済み。
- Workspace/code context: recent Worker runtime registry / spawn boundary work in `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs`; Dashboard/Kanban queue surface in `crates/tui/src/dashboard/mod.rs`; docs/design structure under `docs/design/`。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。visible Pods は current Orchestrator と stopped/restorable historical role Pods only, active child work is none。

IntentPacket:

Intent:
- Kanban UI operation を Backend internal Orchestrator Worker へ安全に接続するための design/planning artifact を作る。Kanban operation は durable orchestration event として記録され、internal Orchestrator は domain-specific tools で event を処理し、実 filesystem-capable work は local/remote runtime 上の Coder/Reviewer/helper Worker へ委譲する境界を明確にする。

Binding decisions / invariants:
- Kanban click / API request から Backend process が直接 shell/git/filesystem を実行しない。
- implementation side effect 前には既存 Ticket lifecycle authority、特に `queued -> inprogress` acceptance と decision record を保持する。
- `ready -> queued` は Orchestrator routing human gate として扱い、unattended scheduler にしない。
- Browser は raw local path / socket path / runtime registry path / raw session path / executable path を authority として渡さない。
- UI display は `worker-name@runtime-name` 形式を使ってよいが、API authority にはしない。
- canonical API identity は `runtime_id` + `worker_id` の runtime-scoped opaque id とする。
- DB design は surrogate worker record id + `UNIQUE(runtime_id, worker_id)` を優先候補とし、run overview / lifecycle event / usage aggregate 参照に使える形にする。
- Backend durable projection は raw transcript 全量ではなく overview / decision / lifecycle / usage aggregate を中心にする。raw session/provider trace は runtime-local source/debug log として bounded read に留める。
- Backend internal Orchestrator の full implementation、Kanban UI completion、remote protocol、raw session full DB ingest、Ticket DB migration、permission/auth completion は non-goal。

Requirements / acceptance criteria:
- Kanban operation から durable orchestration event を生成する方針が明文化される。
- Backend internal Orchestrator Worker の責務 / non-responsibility、domain-specific tool/operation surface、failure/blocker/retry/event ack semantics が整理される。
- WorkerRuntime registry と spawn intent の接続方針が明記される。
- runtime/worker identity, display label, DB identity, event and overview projection boundaries が明確化される。
- filesystem-capable work は runtime 上の Coder/Reviewer/helper Worker に委譲する方針が明記される。
- existing docs/code organization に沿って maintained design artifact が置かれる。

Implementation latitude:
- Primary output は docs/design などの maintained design document でよい。必要に応じて README/index や small comments/tests を追加してよい。
- 既存 Workspace runtime registry / spawn boundary の code names に合わせて wording を調整してよい。
- Full backend implementation や new DB schema migration はしない。

Escalate if:
- Backend process が direct shell/git/filesystem authority を持つ設計にしないと要件を満たせない。
- raw session full ingest / raw socket path / raw workspace path を API authority にする必要が出る。
- `ready -> queued` の human gate を scheduler/lease に置き換える必要が出る。
- API canonical identity を display label や raw Pod name に寄せる必要が出る。
- Ticket DB migration / permission-auth model completion / remote runtime protocol をこの Ticket で固定する必要が出る。

Validation:
- `git diff --check`
- docs-only なら `cargo fmt --check` は不要だが、Rust/comments/tests を触るなら `cargo fmt --check` と relevant `cargo check` / `cargo test` を実施する。
- If `crates/workspace-server` is touched: `cargo test -p yoi-workspace-server` and `cargo check -p yoi`。

Current code/docs map:
- Primary docs: `docs/design/` and docs index/README if appropriate。
- Context code: `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs`, `crates/tui/src/dashboard/mod.rs`。
- Avoid: root/original workspace operations, broad scheduler implementation, Kanban UI completion, remote Host protocol, raw session DB ingest, Ticket storage migration。

Critical risks / reviewer focus:
- accidental scheduler/lease semantics。
- Backend internal Orchestrator gaining raw shell/filesystem/socket authority。
- display labels becoming API authority。
- runtime workspace root / process cwd / worker id / DB surrogate id confusion。
- raw session/provider trace becoming Backend durable projection instead of bounded debug source。
- design doc that repeats Ticket text without resolving event/ack/failure/spawn-intent boundaries.

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVWSQM22-backend-orch-runtime` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T19:07:09Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking relation は 0 件、accepted plan `orch-plan-20260624-190611-1` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- `00001KVWSQM22` は planning/design artifact として implementation-ready であり、Backend internal Orchestrator の full implementation は non-goal として明示した。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVWSQM22-backend-orch-runtime` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T19:08:11Z -->

## Implementation report

Implementation orchestration update:

- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVWSQM22-backend-orch-runtime` と branch `work/00001KVWSQM22-backend-orch-runtime` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVWSQM22-backend-orch-runtime` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。
- この Ticket は full implementation ではなく design/planning artifact として進める。

---

<!-- event: implementation_report author: yoi-coder-00001KVWSQM22-backend-orch-runtime at: 2026-06-24T19:12:33Z -->

## Implementation report

Implementation report:

- 対象: Ticket `00001KVWSQM22`
- 作業ブランチ: `work/00001KVWSQM22-backend-orch-runtime`
- 作業場所: `/home/hare/Projects/yoi/.worktree/00001KVWSQM22-backend-orch-runtime`
- コミット: `f901b9bee37a03faf380894926241107362da360`
- コミットメッセージ: `docs: design backend internal orchestrator runtime`

変更ファイル:

- `docs/design/workspace-kanban-orchestrator-runtime.md`
  - Kanban/API 操作から Backend internal Orchestrator Worker へ接続するための設計文書を追加。
  - durable orchestration event の形、`ack` / `defer` / `fail`、idempotency/restart/retry 方針を整理。
  - Backend internal Orchestrator が routing/control-plane Worker であり、Bash/filesystem/git/raw socket/session authority を持たないことを明記。
  - Coder/Reviewer/helper など filesystem-capable work は typed spawn intent と `WorkerRuntimeRegistry` 経由で委譲する設計として整理。
  - runtime/worker identity、API 形状、DB projection 方針、`display_ref` / `pod_name` が authority ではないことを明記。
  - raw session は通常 projection ではなく bounded debug-only とする境界を整理。
  - dependency/conflict/dirty workspace/missing requirement/runtime unavailable などの waiting/blocker reason を整理。

- `docs/README.md`
  - 新しい設計文書を docs index に追加。

- `docs/design/overview.md`
  - `workspace-server` を local Workspace control-plane seam として位置づけ、raw path/socket/Pod/session を browser/API authority にしない方針を追記。

検証:

- `git diff --check`: success
- docs-only 変更のため `cargo fmt` / `cargo test` / `cargo check` は実行していません。

残リスク / エスカレーション:

- 本 Ticket の意図どおり、設計・計画 artifact の作成までで停止しています。
- Backend internal Orchestrator 本体、Kanban UI 完成、spawn 実装、remote runtime protocol、raw session DB ingest、Ticket storage DB migration、permission/auth completion は未実装のまま non-goal として明記しました。
- merge / close は行っていません。

---

<!-- event: review author: yoi-reviewer-00001KVWSQM22-backend-orch-runtime at: 2026-06-24T19:15:09Z status: approve -->

## Review: approve

Review result: approve

確認範囲:
- child worktree `/home/hare/Projects/yoi/.worktree/00001KVWSQM22-backend-orch-runtime` のみ。
- Ticket `item.md` / `thread.md` / `artifacts/orchestration-plan.jsonl`、commit `f901b9b...` の docs diff、追加 design doc を read-only で確認。
- `git diff --check 8931227..f901b9b -- docs/...`: success。

承認根拠:
- 追加された `docs/design/workspace-kanban-orchestrator-runtime.md` は Ticket の単純コピーではなく、維持対象の設計 artifact として Core rule / durable events / ack-defer-fail / internal Orchestrator tool surface / spawn intent / API identity / DB projection / failure semantics / future implementation order まで具体化している。
- Kanban/API 操作から durable orchestration event、Backend internal Orchestrator Worker、`WorkerRuntimeRegistry`、typed spawn intent、filesystem-capable Coder/Reviewer/helper Worker への委譲までの鎖が明記されている。
- `ready -> queued` は human gate であり unattended scheduler / lease ではない、と明記されている。
- Backend internal Orchestrator は routing/control-plane Worker で、`Bash`、raw filesystem、git/worktree/build、raw socket/session path、raw session full ingest を持たない境界が明確。
- API authority は `runtime_id` + `worker_id` の opaque runtime-scoped identity に限定し、`worker-name@runtime-name` / `display_ref` / `pod_name` / path / socket / session を authority にしない方針が明記されている。
- DB/projection は surrogate `workers.id` + `UNIQUE(runtime_id, worker_id)`、run overview / lifecycle / usage aggregate 参照方針まで記述されている。
- failure/blocker/retry/ack/defer/fail semantics は future implementation Ticket の入力として十分な初期方針になっている。
- Non-goals も維持されており、full backend implementation、Kanban UI completion、remote protocol、raw session DB ingest、Ticket DB migration、permission/auth completion に踏み込んでいない。

Non-blocking notes:
- child worktree 側の Ticket thread には Orchestrator 側で記録した implementation_report が見えなかったが、Orchestrator 側 Ticket record には report を記録済みであり、レビューは commit/diff と Coder report に基づいて行った。
- 新規 design doc は diff 上 EOF newline が無いように見えたが、`git diff --check` は通っており blocker ではない。

---
