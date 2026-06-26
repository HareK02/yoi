<!-- event: create author: "yoi ticket" at: 2026-06-25T16:23:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:30:31Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:30:31Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:31:30Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:32:35Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / workspace state を確認した。
- 本 Ticket は `00001KVZKSV6C` (`Backend RuntimeRegistryの基盤をworker-runtime向けに整理する`) に `depends_on` relation を持つ。
- `00001KVZKSV6C` は現在 `queued` で、さらに `00001KVZBCQH4` worker-runtime core が `inprogress` のため blocked と判断済み。
- Embedded Runtime connection は Backend RuntimeRegistry foundation の handle boundary に依存するため、foundation 確定前に implementation side effect を開始しない。

Evidence checked:
- Ticket body: embedded `worker_runtime::Runtime` registration、direct lib call routing、Backend API exposure、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZKSV6C`; incoming dependent `00001KVZ9JGK0`。
- Orchestration plan: blocker record `orch-plan-20260625-163225-1` を追加。
- Workspace state: `00001KVZBCQH4` は inprogress、`00001KVZKSV6C` は queued/blocked。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZKSV6C` が accepted/completed して Backend Registry foundation が確定した後、再 routing する。

Escalate if:
- Embedded Runtime connection のために `00001KVZKSV6C` の scope/acceptance を変更する必要が出る。
- worker-runtime core API が embedded Backend integration に必要な create/send/projection semantics を満たさない。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-26T05:18:27Z -->

## Decision

Routing decision: implementation_ready

Reason:
- `00001KVZKSV6C` Backend RuntimeRegistry foundation は done。`00001KVZBCQH4` worker-runtime core も done。
- 本 Ticket の scope は embedded Runtime handle を Backend Registry に接続することで、remote Runtime process / FS store / REST command server / event stream / Web Console / config bundle sync は Non-goals。
- 現在 `inprogress` は 0 件。実装 surface は主に `crates/workspace-server` の RuntimeRegistry source/route であり、queued の remote/WebConsole/TUI は本 Ticket 完了後に判断すべき依存関係。

Evidence checked:
- Ticket body: embedded Runtime registration、direct lib call Worker operations、Backend API exposure、Non-goals、acceptance criteria。
- Relations: outgoing dependency `00001KVZKSV6C` は done。incoming `00001KVZ9JGK0` / `00001KW04A8K6` は後続であり blocker ではない。
- Orchestration plan: accepted plan `orch-plan-20260626-051805-2` を記録。
- Workspace state: orchestration worktree clean、WS proxy Ticket は done/cleanup 済み。

IntentPacket:

Intent:
- Workspace Backend の `RuntimeRegistry` に embedded `worker_runtime::Runtime` source を追加し、`backend-internal` Runtime として扱えるようにする。

Binding decisions / invariants:
- Embedded Runtime は HTTP endpoint / token / socket path / session path を持たない。
- Browser-facing API は `runtime_id + worker_id` authority で扱い、embedded Runtime internals / store path / provider credentials を露出しない。
- v0 は memory store / builtin/default Profile fallback / toolsなし Worker でよい。
- Remote Runtime process client、FS store、REST command server、event stream server、Web Console、Profile/config bundle sync は実装しない。
- Local compatibility source は残し、embedded Runtime source と diagnostics / implementation kind で区別する。

Requirements / acceptance criteria:
- Workspace Backend が embedded `worker_runtime::Runtime` を生成・保持し、RuntimeRegistry に登録できる。
- Backend API から `backend-internal` Runtime の summary/status/capabilities を確認できる。
- Registry が embedded Runtime の worker list/detail/create/send input/transcript projection を direct lib call で扱える。
- v0 toolsなし Worker が create でき、input acceptance まで確認できる。
- socket/session/runtime internal store/provider credential が Browser-facing API に出ない。
- Focused workspace-server tests cover embedded runtime registration and routing。

Implementation latitude:
- Existing `/api/workers` / runtime list に含めるか、新 runtime-scoped endpoint に寄せるかは Coder が既存 API shape とテスト容易性に基づいて選び、実装報告で方針を明記する。
- Internal source enum/handle/trait の分割、typed error mapping、test fixture の形は既存 `hosts.rs` / `server.rs` に合わせてよい。

Escalate if:
- `worker-runtime` public API の大幅変更が必要になる。
- Profile/config bundle sync を同時に実装しないと embedded Worker create が成立しない場合。
- Browser-facing API schema を大きく変える必要がある場合。

Validation:
- `cargo fmt --all`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Embedded Runtime internals/path/credentials の漏洩。
- Local compatibility source と embedded Runtime source の混同。
- `runtime_id + worker_id` routing ではなく worker_id-only routing に戻る regression。
- Non-goals（remote/REST/WS/WebConsole/Profile sync）への scope creep。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T05:18:35Z from: queued to: inprogress reason: routing_accepted_after_registry_foundation_done field: state -->

## State changed

Routing decision: implementation_ready。

Dependency `00001KVZKSV6C` Backend RuntimeRegistry foundation と `00001KVZBCQH4` worker-runtime core は done。Ticket body / relations / workspace state / accepted plan を確認し、embedded worker-runtime Registry connection slice は unblocked と判断した。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T05:19:30Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KVZSGT0Q-embedded-runtime-registry`
- Branch: `work/00001KVZSGT0Q-embedded-runtime-registry`
- Coder Worker spawned: `yoi-coder-00001KVZSGT0Q-embedded-runtime`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/workspace-server/**` and root `Cargo.lock` if dependency lock changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Queued capacity note:
- `00001KVZQHPNY` remains queued while this embedded Runtime connection is inprogress to avoid worker creation/profile boundary API churn.
- Remote/WebConsole/TUI dependent Tickets remain queued until embedded/remote prerequisite chain is done.

---

<!-- event: implementation_report author: yoi-coder-00001KVZSGT0Q-embedded-runtime at: 2026-06-26T05:41:14Z -->

## Implementation report

Implementation report:

実装完了。現在のブランチにコミット済み。

Commit:
- `c8209285 feat: add embedded workspace runtime registry`

変更ファイル:
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

実装概要:
- `RuntimeRegistry` に backend-internal な embedded `worker_runtime::Runtime` source を追加。
  - `EmbeddedWorkerRuntime::new_memory(...)`
  - `RuntimeRegistry::for_workspace(...)`
  - `RuntimeRegistry::register(...)`
- local compatibility source は既存の `local-worker-runtime` として維持し、embedded source は `embedded-worker-runtime` として distinct な kind / diagnostics に分離。
- embedded Runtime は browser/API に対して以下を露出しない projection にしている。
  - HTTP endpoint
  - token / credential
  - socket / session path
  - store path
  - provider identity
- Worker 操作は `runtime_id + worker_id` で routing。
  - worker create
  - worker detail
  - input acceptance
  - bounded transcript projection
- v0 として in-memory / tools-less Worker create を実装。
  - Ticket role では builtin profile fallback (`builtin:coder` など)
  - Workspace companion/orchestrator では runtime default profile fallback
- `SocketReady` acceptance は embedded Runtime では明示的に reject。

追加/拡張 API:
- `POST /api/runtimes/{runtime_id}/workers`
- `GET /api/runtimes/{runtime_id}/workers/{worker_id}`
- `POST /api/runtimes/{runtime_id}/workers/{worker_id}/input`
- `GET /api/runtimes/{runtime_id}/workers/{worker_id}/transcript?start=...&limit=...`

既存 API への反映:
- `GET /api/runtimes`
- `GET /api/workers`
にも embedded runtime / embedded worker projection が載る。

テスト:
- embedded runtime registration / routing
- local compatibility source と embedded source の分離
- embedded Worker create
- input acceptance
- transcript projection
- wrong runtime_id での worker 操作が通らないこと
- browser-facing projection に内部 path / credential / socket/session 類が混ざらないこと
- `SocketReady` acceptance が embedded Runtime で reject されること

Validation:
- `cargo fmt --all`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- v0 は intentionally in-memory + tools-less Worker。LLM 実行、remote Runtime process client、FS store、REST command server、event stream server、Web Console、Profile/config sync は未実装。
- embedded runtime の API projection は内部情報を出さない方針だが、diagnostics / evidence / summary JSON に internal identity が混ざっていないか要確認。
- `requested_worker_name` は authority として使わず、embedded Runtime 側の opaque worker id を採用。UX 的表示名が必要なら別 Ticket で profile/config bundle や display metadata の設計が必要。

---

<!-- event: review author: yoi-reviewer-00001KVZSGT0Q-embedded-runtime at: 2026-06-26T05:46:25Z status: approve -->

## Review: approve

Review result: approve

確認結果:
- `RuntimeRegistry` に `LocalWorkerRuntime` と別種別の `EmbeddedWorkerRuntime` が登録され、`backend-internal` Runtime として summary/status/capabilities を返す構成になっている。
- embedded 側は `worker_runtime::Runtime::new_memory` を保持し、worker list/detail/create/send input/transcript は adapter 経由で direct lib call している。REST/remote process/FS store/WebConsole/Profile sync などへの scope creep は見当たらない。
- embedded Runtime summary/worker projection は endpoint/token/socket/session path/store path/provider credential を露出しておらず、capabilities も内部実装詳細を出さない形。
- local compatibility source は `local-pod-runtime`、embedded source は `embedded-worker-runtime` として kind/diagnostics が分離されている。
- API routing は detail/input/transcript が `/api/runtimes/{runtime_id}/workers/{worker_id}` 系で、worker_id-only の操作経路への回帰は見当たらない。
- tests は registration/routing/projection/error/identity boundary を含み、embedded worker 作成・input acceptance・transcript projection と誤 runtime/worker の失敗系も確認されている。

検証:
- read-only 指示に従い、ビルド/テスト再実行やファイル変更は行わず、対象 commit の差分・実装・テストを静的確認した。報告済み validation 結果と矛盾する点はない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T05:47:01Z -->

## Implementation report

Integration / validation report:

Integration:
- Implementation branch `work/00001KVZSGT0Q-embedded-runtime-registry` を orchestration branch に merge 済み。
- Merge commit: `e0cc7acf merge: 00001KVZSGT0Q embedded runtime registry`

Included implementation commit:
- `c8209285 feat: add embedded workspace runtime registry`

Validation in Orchestrator worktree:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success（29 tests passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Review:
- Reviewer approve 済み。embedded Runtime registration/routing、internal path/credential non-leak、local compatibility source separation、runtime_id+worker_id authority に blocker なし。

Outcome:
- Acceptance criteria を満たしたため `done` へ進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T05:47:09Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Backend RuntimeRegistry embedded worker-runtime connection の実装、review、merge、Orchestrator validation が完了した。

Done evidence:
- Merge commit: `e0cc7acf merge: 00001KVZSGT0Q embedded runtime registry`
- Reviewer approve 済み。
- Orchestrator validation:
  - `cargo fmt --all --check`: success
  - `cargo test -p yoi-workspace-server`: success（29 tests passed）
  - `cargo check -p yoi`: success
  - `git diff --check`: success
  - `nix build .#yoi --no-link`: success

Scope:
- Backend RuntimeRegistry に embedded `worker_runtime::Runtime` source を接続。
- Remote process / FS/REST/WS server implementation / Web Console / Profile config sync は Non-goals として未実装。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T05:47:56Z -->

## Implementation report

Cleanup report:

- Child Workers stopped one-by-one and scope reclaimed:
  - `yoi-coder-00001KVZSGT0Q-embedded-runtime`
  - `yoi-reviewer-00001KVZSGT0Q-embedded-runtime`
- Child implementation worktree removed:
  - `/home/hare/Projects/yoi/.worktree/00001KVZSGT0Q-embedded-runtime-registry`
- Child implementation branch removed:
  - `work/00001KVZSGT0Q-embedded-runtime-registry`

Operational note:
- StopPod was executed sequentially, not in parallel.

---
