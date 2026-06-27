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

<!-- event: state_changed author: workspace-panel at: 2026-06-27T19:06:30Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-27T19:08:35Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / orchestration plan / workspace state を確認した。
- 本 Ticket は `00001KW55B32Y` (`worker-runtimeにWorker実行Backend境界を追加する`) に `depends_on` relation を持つ。
- `00001KW55B32Y` は本 routing pass で accepted され `inprogress` になった。
- Adapter は execution backend boundary に接続する必要があるため、boundary の shape が review/merge/done になる前に開始しない。

Evidence checked:
- Ticket body: adapter placement/dependency boundary、Profile/config/authority resolution、input/run lifecycle、protocol event bridge、Non-goals。
- Relations: outgoing `depends_on -> 00001KW55B32Y`; incoming dependent `00001KW55B33H`。
- Orchestration plan: blocker record `orch-plan-20260627-190816-1` を追加。
- Workspace state: `00001KW55B32Y` is inprogress; current worktree clean before implementation side effects.

Next action:
- 本 Ticket は queued のまま待機。
- `00001KW55B32Y` が reviewer approve / merge / validation / done になった後に再 routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-27T20:11:40Z -->

## Decision

Routing decision:

Queue continuation was explicitly requested. The upstream boundary Ticket `00001KW55B32Y` is closed after merge and validation, so this Ticket is now unblocked and accepted for implementation.

Plan:
- Create child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KW55B33B-worker-runtime-worker-adapter`
- Branch:
  - `work/00001KW55B33B-worker-runtime-worker-adapter`
- Route implementation to a sibling Coder Worker with focused write scope.
- Route read-only review to a sibling Reviewer Worker after implementation.
- Orchestrator retains merge, final validation, Ticket completion, child worktree/branch cleanup, and role Pod shutdown.

Review/implementation focus:
- choose adapter placement without introducing crate dependency cycles, and record the rationale
- connect embedded `worker-runtime` execution backend to existing `worker` crate execution
- preserve Profile/config bundle and authority resolution boundaries
- dispatch input to real Worker run lifecycle, not providerless/fake responses
- bridge `protocol::Event` to Runtime observation bus / Backend WS
- return typed diagnostics/rejections for missing config/provider/busy/unsupported operations
- do not expose raw handles, sockets, credentials, session paths, or manifest paths to Browser-facing APIs
- leave `00001KW55B33H` queued until this adapter is merged/validated

Operational note:
- `StopPod` must be called serially during cleanup; do not use parallel StopPod calls.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-27T20:11:44Z from: queued to: inprogress reason: dependency_closed_and_queue_continues field: state -->

## State changed

Queue continuation requested by user. Upstream dependency `00001KW55B32Y` is closed with merge and validation complete, and this Ticket has no remaining blockers. Accepting implementation before child worktree / role Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-27T20:12:31Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW55B33B-worker-runtime-worker-adapter`
- Branch: `work/00001KW55B33B-worker-runtime-worker-adapter`
- Coder Worker spawned: `yoi-coder-00001KW55B33B-worker-adapter`

Scope notes:
- Coder write scope is limited to `crates/worker-runtime/**`, `crates/worker/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Implementation focus:
- adapter placement without crate dependency cycles, with rationale reported back
- connect `worker-runtime` execution backend boundary to existing `worker` crate run lifecycle
- bridge existing `protocol::Event` to Runtime observation bus / Backend WS
- preserve typed diagnostics for missing config/provider/secret and busy/unsupported operations
- avoid fake/providerless responses and Browser-facing raw handle/path/credential/session leaks
- leave downstream Workspace Companion real execution Ticket `00001KW55B33H` queued until this adapter is merged/validated

---

<!-- event: implementation_report author: yoi-coder-00001KW55B33B-worker-adapter at: 2026-06-27T20:51:14Z -->

## Implementation report

Implementation report:

実装完了。マージ / close / cleanup は未実施。

Commit:
- `18526ee36264610048f48b07b5db50ce86852fd2 feat: connect runtime worker execution adapter`

Adapter 配置方針:
- `worker` crate 側に `runtime-adapter` feature 付きで配置。
- 理由:
  - `worker-runtime` は Runtime 境界 / 投影 / Backend trait を持つ下位境界として保ち、実 Worker 実装への依存を持たせない。
  - `worker` crate は既存の `WorkerController` / `WorkerHandle` / `Method::Run` lifecycle を直接扱えるため、fake/canned 応答を挟まずに adapter を組める。
  - `workspace-server` が `worker = { features = ["runtime-adapter"] }` を有効化して embedded Runtime に接続する形にし、crate dependency cycle を避けた。

変更概要:

### `worker` crate

- `crates/worker/src/runtime_adapter.rs` を追加。
- `WorkerRuntimeExecutionBackend` を追加し、`worker_runtime::execution::WorkerExecutionBackend` を実装。
- Runtime worker 作成時に `WorkerController` を spawn し、内部にのみ real `WorkerHandle` を保持。
- Runtime input は `Method::Run { input: vec![Segment::text(...)] }` へ変換して既存 Worker run lifecycle に投入。
- Worker の `protocol::Event` broadcast を購読し、`WorkerExecutionContext::publish_protocol_event` 経由で Runtime observation bus へ橋渡し。
- Provider/profile/config 不足などは spawn/create 時の typed error/rejection として返し、fake assistant text は生成しない。
- v0 double-run policy として queue は持たず、adapter 側 atomic busy flag + Worker status で二重 input を typed busy rejection。
- stop/cancel は既存 `Method::Shutdown` / `Method::Cancel` へ接続。
- Focused test を追加し、fake deterministic LLM client で Runtime input -> real Worker run lifecycle -> assistant output -> `protocol::Event` -> Runtime transcript projection の流れを検証。

### `worker-runtime`

- `observe_worker_event` で受けた real `protocol::Event` から transcript projection を更新。
  - `TextDone` -> Assistant transcript
  - `Error` -> System transcript
  - `ToolResult` -> System transcript
- observation bus には既存 `protocol::Event` をそのまま保持し、subset output model は追加していない。

### `workspace-server`

- embedded Runtime 作成時に `WorkerRuntimeExecutionBackend` を接続。
- `EmbeddedWorkerRuntime::new_memory_with_execution_backend` を追加。
- Browser/API-facing projection に raw `worker::Worker` handle / raw execution handle / socket path / session path / credential / secret ref / manifest path は出していない。

### package/dependency

- `worker` に optional `worker-runtime` dependency + `runtime-adapter` feature を追加。
- `workspace-server` が `worker` の `runtime-adapter` feature を有効化。
- `Cargo.lock` と `package.nix` の `cargoHash` を更新。

Validation:
- `cargo fmt --all`: success
- `cargo fmt --all --check`: success
- `cargo test -p worker --features runtime-adapter runtime_adapter`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- Runtime worker profile 解決は production adapter が通常の Worker profile 解決を使う。workspace-server 起動直後に fake 応答へ fallback せず、profile/provider/config 不足は typed error になる。この挙動が downstream `00001KW55B33H` の Companion UX 設計と合っているか確認が必要。
- Transcript projection は現時点で `TextDone` / `Error` / `ToolResult` を投影。より細かい tool-call lifecycle や thinking block の Browser projection が必要なら downstream で拡張対象。
- Adapter は v0 non-queue policy。二重 input は busy rejection で、background queue/scheduler は実装していない。
- `workspace-server` が `worker` を feature 付きで依存するため、crate layering と public API leak が意図通りかを重点 review してほしい。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-27T20:51:43Z -->

## Implementation report

Review kickoff:

Reviewer Worker spawned for `00001KW55B33B` implementation.

- Reviewer Worker: `yoi-reviewer-00001KW55B33B-worker-adapter`
- Review target commit: `18526ee36264610048f48b07b5db50ce86852fd2 feat: connect runtime worker execution adapter`
- Review mode: read-only implementation worktree review

Review focus:
- crate layering / `runtime-adapter` feature dependency cycle risk
- production Profile/config/authority resolution and typed failure mode
- protocol event bridge into Runtime observation bus / Backend WS
- transcript projection consistency for real Worker run output
- no fake/providerless fallback
- busy / stop / cancel lifecycle semantics
- Browser-facing non-leak of worker handles, raw execution handles, sockets, session paths, credentials, secret refs, and raw manifest paths
- no downstream Workspace Companion bootstrap/UX scope creep

---

<!-- event: review author: yoi-reviewer-00001KW55B33B-worker-adapter at: 2026-06-27T20:58:06Z status: request_changes -->

## Review: request changes

request_changes

読取専用で差分・主要実装を確認した。crate layering 自体は概ね意図通りで、`worker` crate 側の `runtime-adapter` feature に adapter を置き、`worker-runtime` が `worker` に依存しない形は保たれている。また fake/providerless の canned response は見当たらない。

ただし、Ticket 受け入れ条件に対して以下が blocker。

## Blockers

1. **workspace-server の embedded Worker input API がまだ adapter に接続されていない**

   - `workspace-server/src/server.rs:101-113` で adapter は embedded Runtime に install されている。
   - しかし browser/backend registry 側の `EmbeddedWorkerRuntime::send_input` は `hosts.rs:1275-1285` で固定 reject のまま。
   - projection も `hosts.rs:1008-1010` で `can_accept_input: false`, `can_stop: false` のまま。
   - そのため Ticket 要件の「Backend Worker input API からの user input を `Method::Run` 相当に渡す」「Runtime / Backend / Web Console が正規経路を共有する」「embedded Worker への user input が実 Worker run に渡る」を満たしていない。

2. **production profile resolution に builtin 二重 prefix の不整合がある**

   - workspace-server の Ticket role mapping は `hosts.rs:1920-1924` で `ProfileSelector::Builtin("builtin:coder")` のように値へ `builtin:` を含めている。
   - adapter 側は `worker/src/runtime_adapter.rs:133-140` で `Builtin(name)` を常に `format!("builtin:{name}")` しているため、内部的に `builtin:builtin:coder` になる。
   - `manifest::ProfileSelector::parse_cli` は `builtin:builtin:coder` を source=`builtin`, name=`builtin:coder` と解釈するため、通常の builtin `coder` 解決に失敗する。Ticket role Worker の実 spawn path で破綻する可能性が高く、Profile/config resolution の受け入れ条件を満たさない。

3. **run_state/status projection が実 run lifecycle と整合していない**

   - `send_method` は input の `Method::Run` 送信成功時も `WorkerExecutionRunState::Idle` を返している（`worker/src/runtime_adapter.rs:300-313`, `445-455`）。
   - Runtime 側はその `Idle` を input accepted 後の execution status として保存する（`worker-runtime/src/runtime.rs:369-373`）。
   - bridge は `protocol::Event` を observation bus に流しているが、Worker の running/completed/errored/cancelled status を Runtime execution status に反映していない。
   - Ticket 要件の「run started / completed / errored / cancelled を Runtime Worker status と transcript projection に反映」「busy/double-run lifecycle semantics」の観点で不足。

## 確認済み

- `worker-runtime` -> `worker` の依存逆流は見当たらず、adapter placement の方向性は良い。
- `protocol::Event` bridge は `publish_protocol_event` を使って observation bus に流す実装になっている。
- `can_stream_events` / `can_read_bounded_transcript` の public flag 復活は見当たらない。
- downstream `00001KW55B33H` の Companion UX 完成まで踏み込む scope creep は見当たらない。
- Ticket 更新・merge・cleanup・ファイル変更・テスト実行はしていない。Static review のみ。

---

<!-- event: implementation_report author: yoi-coder-00001KW55B33B-worker-adapter at: 2026-06-27T21:22:01Z -->

## Implementation report

Change-response implementation report:

Reviewer blockers を修正済み。マージ / close / cleanup は未実施。

Fix commit:
- `9069b035e0a93f83c702e206d3d15b2b683a8c19 fix: connect embedded runtime input lifecycle`

既存実装 commit:
- `18526ee36264610048f48b07b5db50ce86852fd2 feat: connect runtime worker execution adapter`

## 修正内容

### 1. workspace-server embedded Worker input API を adapter に接続

`EmbeddedWorkerRuntime::send_input` の固定 reject を撤去し、execution backend 接続済みの場合は `worker_runtime::Runtime::send_input` に委譲するようにした。

あわせて:
- `EmbeddedWorkerRuntime` に `execution_enabled` を追加。
- `new_memory_with_execution_backend(...)` で backend 接続済み runtime として扱う。
- Worker projection の capability を backend 接続状態に合わせて更新:
  - `can_accept_input`
  - `can_stop`
- `stop_worker` / `cancel_worker` も embedded Runtime lifecycle API に委譲。
- `workspace-server` 側の focused test を追加し、fake execution backend で embedded Runtime spawn / workspace-server host API 経由 input / backend dispatch / `protocol::Event` / transcript projection / capability/status projection を検証。

### 2. builtin profile selector double-prefix bug 修正

`worker/src/runtime_adapter.rs` の `ProfileSelector::Builtin` 変換を修正。

- `Builtin("coder")` -> `builtin:coder`
- `Builtin("builtin:coder")` -> `builtin:coder`

`builtin:builtin:coder` にならないことを focused unit test で追加確認。

### 3. run_state / status projection を real run lifecycle に合わせて修正

`send_method` が常に `Idle` を返していた点を修正。

- `Method::Run` accepted -> `WorkerExecutionRunState::Busy`
- `Method::Shutdown` accepted -> `Stopped`
- `Method::Cancel` accepted -> `Idle`

さらに `worker-runtime` 側で bridged `protocol::Event` を execution status に投影するようにした。

対応イベント:
- `Status { Running }` -> `Busy`
- `Status { Idle }` -> `Idle`
- `Status { Paused }` -> `Busy`
- `Snapshot { status }` -> status に応じて `Busy` / `Idle`
- `RunEnd { Finished | RolledBack }` -> `Idle`
- `RunEnd { Paused }` -> `Busy`
- `RunEnd { LimitReached }` -> `Errored`
- `Error` -> `Errored`

これにより Backend/Web projection の `status` は execution run_state を反映する。

## Validation

実行済み・成功:
- `cargo fmt --all`
- `cargo fmt --all --check`
- `cargo test -p worker --features runtime-adapter runtime_adapter`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p worker-runtime --features "ws-server fs-store"`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`

補足:
- `cargo test -p yoi-workspace-server` では既存 warning: `field next_sequence is never read`
- 新規失敗や未解決の validation failure はない。

## 残リスク / review focus

- `workspace-server` の embedded Worker capability は backend 接続時に `can_accept_input` / `can_stop` が true になる。実 provider/profile 不足時は fake 応答ではなく typed rejection/error になるため、下流 Companion UX 側でその表示/導線を確認する必要がある。
- Runtime execution status projection は主要 lifecycle event に対応したが、より細かい paused/cancelled 表示語彙を Browser projection で増やす場合は別 Ticket 範囲。
- v0 policy は引き続き queue なし。Busy 中 input は typed busy rejection。

---

<!-- event: review author: yoi-reviewer-00001KW55B33B-worker-adapter at: 2026-06-27T21:27:37Z status: request_changes -->

## Review: request changes

request_changes

静的レビューのみ実施。対象 worktree の HEAD は `9069b035041d17e7c52a454a7563cc5f0b7e1f61` として確認。先回報告の full SHA suffix には差異があるため、以後この full SHA を正とする。

先回の blocker のうち、input delegate / builtin prefix normalization / protocol event -> execution status projection は概ね修正されている。

ただし、元の invariant に対してまだ blocker がある。

## Blocker

**spawn 時の provider/profile/config/setup 失敗が Browser-facing spawn API では `Accepted` 扱いになり、typed rejection/diagnostic になっていない。**

根拠:

- 実 Worker 作成時の profile/provider/session setup 失敗は adapter で `WorkerExecutionSpawnResult::Errored` になる。
  - `worker/src/runtime_adapter.rs:161-194`
  - `worker/src/runtime_adapter.rs:365-371`
- しかし `worker-runtime::Runtime::create_worker` は spawn `Rejected/Errored` を `Err` にせず、`Ok(WorkerDetail)` に execution status として記録するだけ。
  - `worker-runtime/src/runtime.rs:278-280`
  - `worker-runtime/src/runtime.rs:417-446`
- workspace-server 側は `create_worker` が `Ok(detail)` なら常に `WorkerOperationState::Accepted` として返し、execution failure の diagnostic/rejection を browser-facing result に出していない。
  - `workspace-server/src/hosts.rs:1204-1222`
- さらに projection の `can_accept_input` は `run_state != Busy` だけを見るため、spawn `Errored/Rejected` でも status が `Running` のままなら input 可能に見える。
  - `workspace-server/src/hosts.rs:962-970`

このため、missing provider/config/secret/setup が「typed diagnostic/rejection」ではなく「spawn accepted だが内部 execution errored」という中途半端な projection になり、Ticket の受け入れ条件を満たしていない。

spawn execution failure は、少なくとも browser-facing API で sanitized diagnostic/rejection として扱うか、capability を false にして明確な failure projection を返す必要がある。

---

<!-- event: implementation_report author: yoi-coder-00001KW55B33B-worker-adapter at: 2026-06-27T21:39:54Z -->

## Implementation report

Change-response implementation report:

Reviewer blocker を修正済み。Ticket ファイル変更、merge、close、cleanup は未実施。

Current HEAD:
- `7e29ff5ec99dcc748fa5a511cda5bae31fec124b fix: reject embedded spawn execution failures`

Prior fix commit:
- `9069b035041d17e7c52a454a7563cc5f0b7e1f61 fix: connect embedded runtime input lifecycle`

## 修正内容

### Spawn-time execution failure を Browser-facing API で `Rejected` として返す

`worker-runtime::Runtime::create_worker` は spawn backend の `Rejected/Errored` を `Ok(WorkerDetail)` 内の execution status として保持する設計のままなので、`workspace-server` 側で `detail.execution.last_result` を検査するようにした。

対象:
- `crates/workspace-server/src/hosts.rs`

変更:
- embedded Runtime spawn 後に execution spawn failure を検出。
- `WorkerExecutionOutcome::{Rejected, Errored, Unsupported, Busy}` の場合:
  - Browser-facing `WorkerSpawnResult.state = Rejected`
  - `acceptance_evidence = []`
  - sanitized diagnostic を返す
  - worker projection は残すが、capability は失敗状態として false にする

sanitized diagnostic 例:
- code: `embedded_worker_execution_spawn_errored`
- message: `Embedded Worker execution spawn was errored during setup; check runtime configuration`

### Failed execution state で capability が true にならないよう修正

`can_accept_input` / `can_stop` 判定を `run_state != Busy` からより厳密に変更。

`can_accept_input` は以下すべてを満たす場合のみ true:
- execution backend が接続済み
- worker status が `Running`
- execution backend kind が `Connected`
- run_state が `Idle`
- last_result が `Rejected/Errored/Unsupported` ではない

`can_stop` も spawn rejected/errored/unconnected 系では false。

### Focused tests

`workspace-server` に spawn execution failure の focused test を追加。

検証内容:
- fake failing execution backend が spawn `Errored` を返す
- Browser-facing spawn result が `Rejected`
- diagnostic が sanitized され、fake 内部 path を漏らさない
- projected worker status が `errored`
- `can_accept_input == false`
- `can_stop == false`

既存 server API projection test も、real backend setup failure が Browser-facing projection で sanitized rejection として出る前提に調整。

## Validation

実行済み・成功:
- `cargo fmt --all`
- `cargo fmt --all --check`
- `cargo test -p worker --features runtime-adapter runtime_adapter`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p worker-runtime --features "ws-server fs-store"`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`

補足:
- `cargo test -p yoi-workspace-server` では既存 warning: `field next_sequence is never read`
- `nix build` は dirty tree warning を出したが build 自体は成功。

## 残リスク / review focus

- `worker-runtime::Runtime::create_worker` 自体は引き続き spawn execution failure を `Ok(WorkerDetail)` に記録する設計。今回の修正は Browser-facing `workspace-server` projection で typed rejection/diagnostic として露出する対応。
- diagnostic は sanitized のため、具体的な provider/config/secret path 等は Browser-facing API には出さない。詳細原因の調査導線が必要なら、別途 backend/admin 向け diagnostics 設計が必要。
- Companion status は環境の profile/provider setup 可否により `ready` / `error` のどちらも取り得るため、既存テストはその環境差を許容する形に調整。

---

<!-- event: review author: yoi-reviewer-00001KW55B33B-worker-adapter at: 2026-06-27T21:42:24Z status: approve -->

## Review: approve

approve

読取専用で再レビューした。前回 blocker は解消されている。

確認した点:

- `workspace-server` の embedded spawn 後に `detail.execution.last_result` を検査し、`Rejected / Errored / Unsupported / Busy` を browser-facing `WorkerSpawnResult.state = Rejected` として返すようになっている。
- spawn execution failure の diagnostic は sanitized な固定文言・code で返され、内部 path / handle / credential / secret ref の露出は見当たらない。
- `can_accept_input` は backend 接続・Worker `Running`・backend kind `Connected`・`run_state == Idle`・last_result failure でないことを要求しており、spawn failure 後の入力可能 projection は塞がれている。
- `can_stop` も rejected / errored / unconnected failure state では false になる。
- 先回指摘の embedded input API delegate、builtin profile double-prefix、protocol event -> runtime execution status projection も修正済み。
- crate layering は `worker` 側 feature adapter -> `worker-runtime` boundary の向きで、`worker-runtime` が `worker` に依存する逆流は見当たらない。
- fake/providerless fallback、public capability flag 復活、下流 Ticket `00001KW55B33H` scope creep は見当たらない。

検証は指示通り read-only の静的確認のみで、テスト実行やファイル変更はしていない。

---
