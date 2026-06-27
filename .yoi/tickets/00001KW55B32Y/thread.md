<!-- event: create author: "yoi ticket" at: 2026-06-27T18:26:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-27T18:58:48Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-27T18:58:48Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-27T19:06:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-27T19:07:57Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により routing 許可済み。
- 本 Ticket は後続 adapter / Companion 実行 Ticket の前段となる execution backend boundary であり、具体的な `worker` crate adapter 実装は Non-goal として分離されている。
- Outgoing blocking relation はなく、current `inprogress` は 0。orchestration worktree is clean。
- Downstream queued Tickets (`00001KW55B33B`, `00001KW55B33H`) は本 Ticket 完了まで待機させる。

Evidence checked:
- Ticket body: execution backend boundary、input rejection/dispatch、protocol event publish hook、Worker model invariant、Browser-facing non-leak、Non-goals、acceptance criteria。
- Relations: incoming/downstream dependency chain is present via queued Tickets; this Ticket itself has no unresolved outgoing blocker。
- Orchestration plan: accepted plan `orch-plan-20260627-190733-1` recorded。
- Code context: current `worker-runtime::Runtime` / `catalog` / `workspace-server::hosts` have Runtime/Worker model and projection foundation but no concrete execution backend boundary yet。
- Workspace state: queued 3 / inprogress 0、orchestration clean。

IntentPacket:

Intent:
- `worker-runtime` に Worker execution backend 境界を追加し、Runtime が input-capable Worker と backend-unconnected placeholder を混同しないようにする。

Binding decisions / invariants:
- Execution backend 未接続 Worker への user input は accepted にしない。
- Runtime 自身が fake / providerless assistant response を生成しない。
- Worker transcript / observation は正規 contract とし、`can_stream_events` / `can_read_bounded_transcript` を public capability として復活させない。
- Execution backend handle / socket path / credential / session path / raw worker handle は Browser-facing API に出さない。
- 既存 `worker` crate への具体 adapter 接続、Workspace Companion real LLM execution、Web Console UX redesign は実装しない。

Requirements / acceptance criteria:
- `worker-runtime` に execution backend trait / handle / enum / lifecycle contract がある。
- backend connected Worker の input dispatch 境界と run state/busy/rejected/errored typed result がある。
- stop/cancel unsupported は typed rejection。
- protocol event を Runtime observation bus へ publish する hook がある。
- Focused tests が backend 未接続 input rejection、connected backend input dispatch boundary、observation publish hook を確認する。
- `cargo test -p worker-runtime --features ws-server`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi`, `git diff --check`, `nix build .#yoi --no-link` が通る。

Implementation latitude:
- Backend boundary の具体型名、trait sync/asyncの形、test backend/fake backend の実装、Runtime state/projectionの最小変更は Coder が既存 `worker-runtime` design に合わせて選べる。
- `workspace-server` projection に必要な最小 diagnostic/state 表現は追加してよい。
- 既存 `worker` crate adapter に必要そうな shape は見越してよいが、実 adapter 実装はしない。

Escalate if:
- `worker` crate に大きく手を入れないと boundary 自体が定義できない。
- Browser-facing API に raw execution handle/path/credential を出す必要が出る。
- public capability として `can_stream_events` / `can_read_bounded_transcript` を戻さないと UI/API が成立しない。
- fake/providerless response を再導入しないと tests/UX が通らない。

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- backend 未接続 Worker が input accepted になる regression。
- Runtime-generated fake/providerless assistant response の再導入。
- Browser-facing projection に execution backend internals/path/credential/session が漏れること。
- Worker transcript/observation を optional display capability として扱うこと。
- 後続 adapter Ticket の scope を先取りしすぎること。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-27T19:08:06Z from: queued to: inprogress reason: routing_accepted_execution_backend_boundary field: state -->

## State changed

Routing decision: implementation_ready。

Ticket body / relation / workspace state / accepted plan を確認し、Worker execution backend boundary slice は unblocked と判断した。Downstream queued Tickets は本 Ticket の completion を待つ。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-27T19:09:41Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW55B32Y-worker-runtime-execution-backend`
- Branch: `work/00001KW55B32Y-worker-runtime-execution-backend`
- Coder Worker spawned: `yoi-coder-00001KW55B32Y-exec-backend`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/worker-runtime/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Queued dependency notes:
- `00001KW55B33B` remains queued while this execution backend boundary is inprogress.
- `00001KW55B33H` remains queued behind `00001KW55B33B`.

---

<!-- event: implementation_report author: yoi-coder-00001KW55B32Y-exec-backend at: 2026-06-27T19:34:20Z -->

## Implementation report

Implementation report:

実装完了。マージ / close / cleanup は未実施。

Commit:
- `2d5971738478f832ba9a135601ea11dda60c565d feat: add worker execution backend boundary`

変更概要:

### `worker-runtime`

追加/変更:
- `crates/worker-runtime/src/execution.rs` を追加。
  - `WorkerExecutionBackend` trait
  - `WorkerExecutionHandle`
  - `WorkerExecutionContext`
  - `WorkerExecutionStatus`
  - `WorkerExecutionRunState`
  - `WorkerExecutionResult`
  - `WorkerExecutionOperation`
  - `WorkerExecutionOutcome`
  - `WorkerExecutionSpawnRequest`
  - `WorkerExecutionSpawnResult`
- `Runtime::with_execution_backend(...)` を追加。
- `Runtime::with_fs_store_and_execution_backend(...)` を追加。
- `WorkerSummary` / `WorkerDetail` に `execution` status を追加。
- `Runtime::create_worker(...)`
  - backend 接続時は spawn/initialize 境界を呼ぶ。
  - backend 未接続時は `execution.backend = unconnected`。
- `Runtime::send_input(...)`
  - backend 未接続 Worker への input を `WorkerExecutionUnavailable` で拒否。
  - backend dispatch が `Busy` / `Rejected` / `Errored` / `Unsupported` を返した場合、typed `WorkerExecutionRejected` として拒否。
  - dispatch accept 後のみ transcript / event / observation を通常 Worker contract として記録。
- `Runtime::stop_worker(...)` / `cancel_worker(...)`
  - active かつ connected handle ありの場合は backend lifecycle 境界を呼ぶ。
  - default backend implementation は unsupported typed rejection。
- backend から Runtime observation bus へ `protocol::Event` を publish できる hook を追加。
- HTTP API error mapping に execution unavailable/rejected を追加。

### `workspace-server`

- Embedded Worker input unavailable diagnostic を LLM 固有表現から execution backend 境界表現へ変更。
  - `embedded_worker_execution_unavailable`
- RuntimeRegistry / Backend projection のテストで、Browser-facing JSON に以下が出ないことを確認。
  - `can_stream_events`
  - `can_read_bounded_transcript`
  - raw URL/path/socket/session/credential/provider 系の既存禁止項目。

Validation:
- `cargo fmt --all`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- concrete `worker` crate adapter は意図通り未実装。下流 Ticket `00001KW55B33B` 側で今回の `WorkerExecutionBackend` trait に接続する想定。
- Embedded Runtime は現時点では backend-unconnected placeholder のままなので、workspace-server 側では input unavailable diagnostic を返す。
- backend trait は sync 境界として実装。実 adapter 実装時に async/process lifecycle、busy state、cancel semantics のレビューが必要。
- `WorkerSummary` / `WorkerDetail` の public JSON に `execution` が増えるが、handle/path/socket/credential/session/manifest など raw internals は含めていない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-27T19:36:13Z -->

## Implementation report

Review kickoff:

Reviewer Worker spawned for `00001KW55B32Y` implementation.

- Reviewer Worker: `yoi-reviewer-00001KW55B32Y-exec-backend`
- Review target commit: `2d5971738478f832ba9a135601ea11dda60c565d feat: add worker execution backend boundary`
- Review mode: read-only implementation worktree review

Review focus:
- backend-unconnected input rejection and no transcript/event acceptance
- connected fake backend input dispatch boundary
- protocol event publish hook correctness
- stop/cancel unsupported typed rejection
- no fake/providerless assistant response
- no `can_stream_events` / `can_read_bounded_transcript` public capability regression
- no raw execution handle/path/socket/session/credential leakage
- no scope creep into concrete `worker` crate adapter or Companion real LLM execution

---

<!-- event: review author: yoi-reviewer-00001KW55B32Y-exec-backend at: 2026-06-27T19:41:12Z status: request_changes -->

## Review: request changes

request_changes

ブロッカー:

- `worker-runtime` の既存 feature 組み合わせ `ws-server + fs-store` がコンパイル不能。今回追加された observation bus 用フィールドが `RuntimeState::from_persisted` で初期化されていない。
  - 実行: `CARGO_TARGET_DIR=/tmp/yoi-review-target-00001KW55B32Y cargo check -p worker-runtime --features "ws-server fs-store"`
  - 失敗: `crates/worker-runtime/src/runtime.rs:925`
  - エラー: missing fields `next_observation_sequence`, `observation_events`, `observation_tx` in initializer of `RuntimeState`
  - 既存の fs-store runtime path と ws observation path の組み合わせを壊す regression なので修正が必要。

確認した範囲では、Ticket の主眼自体は概ね実装されている:

- unconnected Worker input は typed `WorkerExecutionUnavailable` で拒否され、user transcript / observation event は追加されない。
- connected fake backend による input dispatch test がある。
- `protocol::Event` を observation bus に publish する hook と test がある。
- stop/cancel unsupported は typed rejection になる。
- fake/providerless/canned assistant response 生成は見当たらない。
- `can_stream_events` / `can_read_bounded_transcript` の public capability 復活は見当たらない。
- Browser-facing projection で raw backend handle/socket/session/credential/path を漏らす変更は見当たらない。
- concrete `worker` crate adapter / Companion real LLM execution への scope creep は見当たらない。

追加検証:

- `git diff --check HEAD~1..HEAD`: pass
- 作業ツリーは review 後も clean。

---
