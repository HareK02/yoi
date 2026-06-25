<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:03Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-25T16:27:28Z -->

## Decision

Decision update: split Backend Runtime work into three implementation tickets.

1. 00001KVZKSV6C Backend RuntimeRegistryの基盤をworker-runtime向けに整理する
   - Registry identity/projection/error boundary only.
   - No embedded Runtime handle implementation.
   - No remote Runtime client implementation.
2. 00001KVZSGT0Q Backend RuntimeRegistryにembedded worker-runtimeを接続する
   - Direct lib-call integration for backend-internal Runtime.
3. 00001KVZSGT14 Backend RuntimeRegistryにremote worker-runtime processを接続する
   - HTTP/event-stream client integration for remote Runtime processes.

Companion Web Console depends on embedded integration, not remote integration.


---

<!-- event: intake_summary author: hare at: 2026-06-25T16:30:31Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:30:31Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:31:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:32:17Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / workspace state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress`。Coder implementation report は届いているが、まだ review / merge / validation / done ではない。
- 本 Ticket の要件は worker-runtime core domain types を前提に Backend Registry foundation を整理する内容なので、依存 Ticket が確定する前に別 worktree / Coder Worker を開始すると API churn / conflict risk が高い。

Evidence checked:
- Ticket body: Backend RuntimeRegistry foundation、runtime_id / worker_id authority、existing local compatibility source、workspace-server hosts/server target、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`; incoming dependents `00001KVZSGT0Q`, `00001KVZSGT14`。
- Orchestration plan: blocker record `orch-plan-20260625-163206-1` を追加。
- Workspace state: `00001KVZBCQH4` implementation worktree exists and coder has reported commit `9b2cae32`, but no reviewer approval / Orchestrator merge yet。
- Queue state: 本 Ticket と `00001KVZSGT0Q` が queued、`00001KVZBCQH4` が inprogress。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing して unblocked なら `queued -> inprogress` acceptance に進む。

Escalate if:
- `00001KVZBCQH4` の Runtime API shape が本 Ticket の前提を満たさない。
- Backend Registry foundation 側で worker-runtime core の追加変更が必要になる。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:55:09Z -->

## Decision

Routing decision: implementation_ready

Reason:
- `00001KVZBCQH4` worker-runtime core は done。dependency blocker は解消済み。
- 本 Ticket の目的は Backend RuntimeRegistry foundation の整理で、embedded runtime 接続や REST/FS 実装そのものではない。
- queued/inprogress 再確認時点で `inprogress` は 0 件。FS store Ticket とは主変更面が `crates/workspace-server` vs `crates/worker-runtime` feature で概ね分離できるため、本 Ticket は受理可能。

Evidence checked:
- Ticket body: Backend RuntimeRegistry foundation、runtime_id / worker_id authority、existing local compatibility source、workspace-server hosts/server target、Non-goals。
- Relations: outgoing dependency `00001KVZBCQH4` は done。incoming dependent `00001KVZSGT0Q`, `00001KVZSGT14` は後続であり blocker ではない。
- Orchestration plan: accepted plan `orch-plan-20260625-165451-2` を記録。
- Workspace state: orchestration worktree clean。worker-runtime core merge/validation/done/cleanup 済み。

IntentPacket:

Intent:
- Backend `RuntimeRegistry` の domain boundary を、legacy process/source abstraction から worker-runtime を受けられる形へ整理する。

Binding decisions / invariants:
- Backend Registry の authority は Runtime / Worker domain identity を扱い、raw socket/session/path/pod name を public authority にしない。
- この Ticket では embedded `worker_runtime::Runtime` を実際に接続しない。handle/trait/enum/boundary と diagnostics/projection 整理まで。
- Remote Runtime HTTP client / REST server / Web Console / dynamic Runtime registration は実装しない。
- 既存 local compatibility source の behavior は壊さない。
- `worker-runtime` core crate の API を大きく変更しない。必要になれば escalation。

Requirements / acceptance criteria:
- `workspace-server` の RuntimeRegistry foundation が worker-runtime 向けの identity / projection / error boundary を持つ。
- Existing local/process compatibility source と将来 embedded/remote source の境界が型で分かる。
- Browser/API response へ socket/session/path authority を露出しない。
- Existing workspace-server tests が通る。

Implementation latitude:
- Type/module split、内部 trait/enum naming、diagnostic representation は既存 `crates/workspace-server/src/hosts.rs` の形に合わせてよい。
- Minimal compile/test slice でよい。実 embedded handle は placeholder/boundary でよい。

Escalate if:
- `worker-runtime` crate の public API 変更が必要になる。
- Backend embedded connection を実装しないと acceptance を満たせないと判明する。
- API response schema 互換性や dashboard expectations を大きく変える必要がある。

Validation:
- `cargo fmt --all`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Registry boundary が legacy Pod/socket/session authority を引きずること。
- `00001KVZSGT0Q` の実 embedded接続まで scope creep すること。
- Existing local compatibility source の API/diagnostics regression。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T16:55:16Z from: queued to: inprogress reason: routing_accepted_after_worker_runtime_core_done field: state -->

## State changed

Routing decision: implementation_ready。

Dependency `00001KVZBCQH4` worker-runtime core は done。Ticket body / relations / workspace state / orchestration plan を確認し、Backend RuntimeRegistry foundation は unblocked と判断した。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T16:58:32Z -->

## Implementation report

Implementation startup blocked after acceptance:

- Worktree created:
  - `/home/hare/Projects/yoi/.worktree/00001KVZKSV6C-backend-runtime-registry`
  - branch `work/00001KVZKSV6C-backend-runtime-registry`
- Coder Worker spawn attempted:
  - `yoi-coder-00001KVZKSV6C-runtime-registry`
- Spawn failed before child socket appeared:
  - `yoi: unknown command pod`

Interpretation:
- No implementation code changes have been made in this worktree yet.
- This appears to be a runtime/launcher compatibility issue after the Pod→Worker CLI rename: the SpawnPod tool path attempted to invoke a `pod` subcommand that the child-launch binary rejected.
- Because this failure happened after `queued -> inprogress` acceptance and worktree creation, the Ticket remains inprogress with a recovery note rather than being silently returned to queued.

Recovery needed:
- Restart/restore Orchestrator on a consistent runtime binary or repair the launcher compatibility path, then retry Coder Worker spawn for this accepted worktree.
- If retry remains impossible, use an explicit human-authorized alternative implementation route.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T19:24:51Z -->

## Implementation report

Implementation recovery / kickoff:

- User reported the previous SpawnPod failure was caused by a development environment variable forcing child launch from `target` binary. The variable was removed and installed binary is now used.
- Retried Coder Worker spawn for the already accepted worktree; spawn succeeded.

Active implementation:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVZKSV6C-backend-runtime-registry`
- Branch: `work/00001KVZKSV6C-backend-runtime-registry`
- Coder Worker: `yoi-coder-00001KVZKSV6C-runtime-registry`
- Scope: read implementation worktree + non-recursive workspace root, write `crates/workspace-server/**` only.

Next action:
- Wait for Coder implementation report, then route to reviewer.

---

<!-- event: implementation_report author: yoi-coder-00001KVZKSV6C-runtime-registry at: 2026-06-25T19:33:11Z -->

## Implementation report

Implementation report:

実装完了。Ticket ファイル/project records は変更していない。merge/close/cleanup 前で停止。

Commit:
- `f6fd7b63 feat: add workspace runtime registry source boundary`

変更ファイル:
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

実装/API 概要:
- `WorkerRuntimeRegistry` を `RuntimeRegistry` に改名し、workspace-server 内の Runtime/Worker domain 側の registry 名を明確化。
- `RuntimeSummary` に `source: RuntimeSourceSummary` を追加。
  - `RuntimeSourceKind`: `local_compatibility`, `embedded_worker_runtime`, `remote_http`
  - `RuntimeSourceStatus`: `active`, `reserved`
  - `RuntimeIdentityAuthority`: `runtime_registry_projection`
- 現行 local Pod metadata 由来の互換 source は `local_compatibility` として明示。
- embedded worker-runtime / remote HTTP は reserved boundary として型だけ用意し、実接続・HTTP client/server・dynamic registration は未実装のまま保持。
- API response に source boundary / identity authority を追加し、Runtime public identity が socket/session/path ではなく registry projection であることをテストで確認。
- 既存 local compatibility behavior は維持し、既存テストに source boundary の assert を追加。

Validation:
- `cargo fmt --all`: success
- `cargo test -p yoi-workspace-server`: success（19 passed）
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success（dirty warning は出たが build 成功）

Residual risks / review focus:
- API top-level `source: "worker_runtime_registry"` は既存互換のため維持。一方で Runtime item 側に typed `source.kind` / `identity_authority` を追加。
- embedded/remote source は reserved constructor と enum boundary のみで、実際の registry composition や Runtime 接続は次 Ticket 側の責務。
- `RuntimeSourceSummary.note` は human-readable な API payload なので、Web 側で表示するなら文言の粒度を確認してほしい。

---

<!-- event: review author: yoi-reviewer-00001KVZKSV6C-runtime-registry at: 2026-06-25T19:37:41Z status: request_changes -->

## Review: request changes

Review result: request_changes

ブロッカー:

1. Runtime/Worker authority がまだ `runtime_id + worker_id` になっていない。
   - `RuntimeRegistry::worker(&self, worker_id)` は `runtime_id` を受け取らず、全 Runtime を `worker_id` だけで走査している。
   - `RuntimeRegistryError` にも `UnknownRuntime` 相当がない。
   - Ticket の binding decision / 要件では Worker authority は `runtime_id + worker_id`、unknown runtime / unknown worker 等の error boundary を typed に分ける必要がある。
   - 将来 embedded / remote source が増えた時に `worker_id` 衝突や誤 routing が起き得るため、foundation として不足。
   - 該当: `crates/workspace-server/src/hosts.rs` `RuntimeRegistry::worker` / `RuntimeRegistryError`

2. runtime-scoped routing/error mapping のテストが不足している。
   - 追加テストは `local_compatibility` source / identity_authority / path 非露出は確認しているが、複数 Runtime または unknown runtime を想定した `runtime_id` scoped lookup / error boundary を確認していない。
   - Acceptance の “Runtime identity / Worker routing key / error boundary” を満たすには、typed `runtime_id` lookup と unknown-runtime / unknown-worker の focused test が必要。

確認できた良い点:
- `RuntimeSourceKind::{LocalCompatibility, EmbeddedWorkerRuntime, RemoteHttp}` と `RuntimeSourceSummary` により、local compatibility / future embedded / future remote の source 境界は型として追加されている。
- local source は `source.kind = local_compatibility`、`identity_authority = runtime_registry_projection` として API/test で確認されている。
- socket/session/path authority の直接公開や embedded `worker_runtime::Runtime` 接続、remote HTTP client/server 実装などの scope creep は見当たらない。
- 変更範囲は `workspace-server` の `hosts.rs` / `server.rs` のみで、`worker-runtime` public API 変更は見当たらない。
- read-only で `git diff --check f6fd7b63^ f6fd7b63` は問題なし。Cargo/Nix 検証は read-only 指示を優先して再実行していない。

---
