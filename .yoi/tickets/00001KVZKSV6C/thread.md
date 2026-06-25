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
