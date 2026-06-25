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

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:39:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:44:20Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / queue state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress` で、implementation commits はあるが reviewer review 中。merge / Orchestrator validation / done ではない。
- REST command server は worker-runtime command/response/projection semantics に依存するため、core API 確定前に HTTP/server implementation side effect を開始しない。

Evidence checked:
- Ticket body: REST command server、request/response schema、operation timeouts、host/port config、diagnostics、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`。incoming dependent `00001KVZ9JGK0`。
- Orchestration plan: blocker record `orch-plan-20260625-164410-1` を追加。
- Queue state: queued は本 Ticket、FS store、Backend Registry foundation、Backend embedded connection の4件。inprogress は `00001KVZBCQH4` 1件。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing する。

Escalate if:
- worker-runtime core の command/projection API が REST mapping に足りない。
- REST server concerns を core crate に混ぜないと acceptance を満たせないように見える。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T20:35:55Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue された dependent chain を再確認した。
- `00001KVZBCQH4` worker-runtime core は done、`00001KVZKST83` FS store feature も done/merged/validated 済み。以前の dependency/capacity blocker は解消した。
- 本 Ticket は REST command server であり、Non-goals に SSE / WebSocket event stream server が明記されている。WebSocket transport decision は `00001KVZKSTJT` 側で扱い、本 Ticket の implementation latitude には含めない。
- 現在の inprogress `00001KVZKSV6C` は `crates/workspace-server` foundation、こちらは `crates/worker-runtime` http-server feature が主対象で、conflict risk は bounded。

Evidence checked:
- Ticket body: `http-server` feature、Runtime process wrapper、REST command endpoints、typed JSON/errors、Browser direct Runtime access exclusion、Non-goals。
- Relations: outgoing dependency `00001KVZBCQH4` は done。incoming `00001KVZKSTJT`, `00001KVZSGT14` は後続。
- Orchestration plan: accepted plan `orch-plan-20260625-203533-3` を記録。
- Workspace state: orchestration worktree clean; current active foundation branch is separate surface.

IntentPacket:

Intent:
- `worker-runtime` に optional `http-server` feature と最小 Runtime process REST command API を追加する。

Binding decisions / invariants:
- REST handlers は `Runtime` lib API を呼ぶ wrapper とし、Worker semantics を二重実装しない。
- Browser は Runtime process に直接接続せず、Backend 経由の前提を docs/comments/API comments に残す。
- SSE / WebSocket event stream server、Backend HTTP client integration、dynamic Runtime registration、Web Console、full auth model は実装しない。
- `http-server` disabled 時に core library は HTTP server dependency を強制しない。
- Runtime authority は Runtime/Worker identity。legacy pod/socket/session path を public REST authority として設計しない。

Requirements / acceptance criteria:
- `http-server` feature と binary/process wrapper がある。
- `GET /v1/runtime`, `GET /v1/workers`, `GET /v1/workers/{worker_id}`, `POST /v1/workers`, `POST /v1/workers/{worker_id}/input`, `stop`, `cancel`, `GET transcript` を扱う。
- typed request/response/error shapes を持つ。
- runtime id / bind address / store selection を v0 config として扱える。
- minimal local token placeholder は可。ただし Browser に Runtime credential を渡す前提にしない。

Implementation latitude:
- HTTP framework/dependency、binary/module split、test helper、typed response shapes の詳細は Coder が選べる。
- FS store 使用は既存 feature を使う範囲まで。新しい persistence design は不要。

Escalate if:
- WebSocket/SSE observation 実装が必要になりそうな場合。
- Backend integration や dynamic runtime registration を同時に実装しないと REST command server が成立しない場合。
- Core Runtime API の大幅変更が必要になる場合。

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime --no-default-features`
- `cargo test -p worker-runtime --features http-server`
- 必要に応じて `cargo test -p worker-runtime --features fs-store,http-server`
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Feature gating/dependency leakage。
- REST handler が Runtime semantics を複製すること。
- Browser direct Runtime access や credential leakage。
- WebSocket/SSE scope creep。
- package.nix cargoHash / lock consistency。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T20:36:02Z from: queued to: inprogress reason: routing_accepted_after_core_and_fs_store_done field: state -->

## State changed

Routing decision: implementation_ready。

Dependency `00001KVZBCQH4` worker-runtime core は done。以前の waiting-capacity reason だった FS store branch も done/merged/validated 済み。Ticket body / relations / workspace state / accepted plan を確認し、REST command server slice は unblocked と判断した。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---
