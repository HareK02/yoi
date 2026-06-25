<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:34:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:34:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:39:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:44:04Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / queue state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress` で、implementation commits はあるが reviewer review 中。merge / Orchestrator validation / done ではない。
- FS persistence feature は worker-runtime core API / store boundary に依存するため、core API 確定前に開始しない。

Evidence checked:
- Ticket body: `fs-store` feature、atomic write、schema/version metadata、diagnostics、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`。
- Orchestration plan: blocker record `orch-plan-20260625-164354-1` を追加。
- Queue state: queued は本 Ticket、REST server、Backend Registry foundation、Backend embedded connection の4件。inprogress は `00001KVZBCQH4` 1件。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing する。

Escalate if:
- `00001KVZBCQH4` の store/allocation boundary が FS feature の requirements を満たさない。
- FS feature を core Ticket に巻き戻す必要が出る。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:55:42Z -->

## Decision

Routing decision: implementation_ready

Reason:
- `00001KVZBCQH4` worker-runtime core は done。dependency blocker は解消済み。
- 本 Ticket は optional FS persistence feature であり、REST server / Backend Registry integration とは分離されている。
- queued/inprogress 再確認時点で `00001KVZKSV6C` を受理したが、主変更面は Backend Registry foundation (`crates/workspace-server`) と FS feature (`crates/worker-runtime`) で分離できる。Cargo/package files の機械的 conflict は Orchestrator merge時に解消可能と判断する。

Evidence checked:
- Ticket body: `fs-store` feature、runtime/worker scoped layout、atomic write、corrupt diagnostics、bounded reads、Non-goals。
- Relations: outgoing dependency `00001KVZBCQH4` は done。incoming remote Runtime Ticket は後続であり blocker ではない。
- Orchestration plan: accepted plan `orch-plan-20260625-165525-2` を記録。
- Workspace state: orchestration worktree clean。worker-runtime core merge/validation/done/cleanup 済み。

IntentPacket:

Intent:
- `worker-runtime` に optional `fs-store` feature と filesystem persistence backend を追加する。

Binding decisions / invariants:
- Feature disabled 時に core library は FS store dependency を強制しない。
- Store authority は `runtime_id + worker_id`。legacy pod path / socket path / session path を authority にしない。
- Standalone `worker-store` crate は作らない。`worker-runtime` 内 feature/module として実装する。
- REST command server / event stream / Backend integration / legacy Pod session migration / SQLite store は実装しない。
- Existing memory store/core tests を壊さない。

Requirements / acceptance criteria:
- `fs-store` feature がある。
- `cargo test -p worker-runtime --no-default-features` が通る。
- `cargo test -p worker-runtime --features fs-store` が通る。
- Worker create/state update/event append/transcript append/bounded read が FS store で動く。
- Runtime/Worker scoped layout、atomic write、directory creation、corrupt/missing diagnostics を扱う。

Implementation latitude:
- FS layout、module split、serialization structs、atomic write helper、diagnostics typeの詳細は Coder が既存 style に合わせて選べる。
- 旧 `pod-store` の atomic write pattern は参考にしてよいが、依存/責務移植は不要。

Escalate if:
- core Runtime store abstraction の大幅 rewrite が必要になる。
- REST/Backend integration を同時実装しないと acceptance を満たせないと判明する。
- `worker-store` standalone crate が必要に見える。

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime --no-default-features`
- `cargo test -p worker-runtime --features fs-store`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Feature gating failure causing core to pull FS dependencies。
- FS layout leaking legacy Pod/session authority。
- Atomic write / corrupt record handling が不十分。
- Memory store semantics regression。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T16:55:50Z from: queued to: inprogress reason: routing_accepted_after_worker_runtime_core_done field: state -->

## State changed

Routing decision: implementation_ready。

Dependency `00001KVZBCQH4` worker-runtime core は done。Ticket body / relations / workspace state / orchestration plan を確認し、optional FS store feature は unblocked と判断した。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T16:58:41Z -->

## Implementation report

Implementation startup blocked after acceptance:

- Worktree created:
  - `/home/hare/Projects/yoi/.worktree/00001KVZKST83-worker-runtime-fs-store`
  - branch `work/00001KVZKST83-worker-runtime-fs-store`
- Coder Worker was not spawned because the immediately preceding SpawnPod attempt for parallel accepted Ticket `00001KVZKSV6C` failed with:
  - `yoi: unknown command pod`

Interpretation:
- No implementation code changes have been made in this worktree yet.
- This appears to be the same runtime/launcher compatibility issue after the Pod→Worker CLI rename.
- Because this Ticket was already accepted (`queued -> inprogress`) and worktree creation completed, it remains inprogress with a recovery note rather than being silently returned to queued.

Recovery needed:
- Restart/restore Orchestrator on a consistent runtime binary or repair the launcher compatibility path, then spawn the Coder Worker for this accepted worktree.
- If retry remains impossible, use an explicit human-authorized alternative implementation route.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T19:24:59Z -->

## Implementation report

Implementation recovery / kickoff:

- User reported the previous SpawnPod failure was caused by a development environment variable forcing child launch from `target` binary. The variable was removed and installed binary is now used.
- Retried Coder Worker spawn for the already accepted worktree; spawn succeeded.

Active implementation:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVZKST83-worker-runtime-fs-store`
- Branch: `work/00001KVZKST83-worker-runtime-fs-store`
- Coder Worker: `yoi-coder-00001KVZKST83-fs-store`
- Scope: read implementation worktree + non-recursive workspace root, write `crates/worker-runtime/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` only.

Next action:
- Wait for Coder implementation report, then route to reviewer.

---
