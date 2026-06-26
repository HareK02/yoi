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
