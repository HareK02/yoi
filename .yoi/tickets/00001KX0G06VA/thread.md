<!-- event: create author: "yoi ticket" at: 2026-07-08T09:12:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-08T09:27:52Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-08T09:27:52Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-08T10:04:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-08T10:05:32Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Ticket 自体は Runtime-to-Backend resource fetch REST API として実装 intent / acceptance criteria が見えるが、Ticket body は `00001KWZ5KERY` Decodal ProfileSourceArchive work の前段として扱う順序を明記している。
- 現在 `00001KWZ5KERY` はすでに `inprogress` で、implementation branch `work/00001KWZ5KERY-decodal-profile-archive` があり、外部 review 待ち。
- 両 Ticket は worker-runtime / workspace-server / ProfileSourceArchive prefetch/verify の同一 surface に触れるため、今この Ticket を別 branch で開始すると高確率で conflict し、active Decodal review の前提を壊す。
- したがってこの routing pass では `queued -> inprogress` を記録せず、worktree 作成 / Pod spawn などの implementation side effect は行わない。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0G06VA)`: typed relation 0 件。
- `TicketOrchestrationPlanQuery(00001KX0G06VA)`: prior record 0 件だったため、今回 `before 00001KWZ5KERY` と waiting-capacity note を記録。
- `TicketList`: queued はこの Ticket 1件、inprogress は `00001KWZ5KERY` 1件。
- Orchestrator worktree git status: clean on `orchestration`。
- `00001KWZ5KERY` implementation branch exists and is under review。

Next action:
- `00001KWZ5KERY` の review 結果を待つ。
- review が request_changes で resource-fetch API prerequisite が必要と確認された場合、または Decodal branch をどう扱うかの integration-order decision が明確になった後、この Ticket を再 routing して start する。
- Decodal branch が approve された場合も、この Ticket を後続で必要とするか、Decodal implementation を resource-fetch API に合わせて follow-up refactor するかを明示的に判断してから開始する。

Escalate if:
- active Decodal branch を中断/rebase/drop して、この Ticket を先に実装する方針に切り替える必要がある場合。
- resource-fetch API の public/auth/capability model が Decodal Ticket の recorded invariants を変える必要がある場合。

---

<!-- event: decision author: orchestrator at: 2026-07-08T10:38:51Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 前回この Ticket を queued のまま保持した理由は、`00001KWZ5KERY` が inprogress/review 中で、同一 worker-runtime / workspace-server / ProfileSourceArchive prefetch surface に触れるためだった。
- 現在 `00001KWZ5KERY` は closed になり、implementation branch も orchestration branch に merge 済みで、previous blocker は解消している。
- この Ticket は Runtime -> Backend resource fetch REST API / resource handle / broker / Runtime client/cache / authority-boundary tests の intent と acceptance criteria が具体化されている。
- typed relation blocker は 0 件。既存 OrchestrationPlan の `before 00001KWZ5KERY` / waiting note は過去の並列停止理由として確認したが、Decodal 側が完了済みのため現在の acceptance blocker ではない。
- queued notification と今回の user follow-up「ないなら順当に消化して」により、human authorized routing/start の条件を満たす。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0G06VA)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KX0G06VA)`: prior `before 00001KWZ5KERY` と waiting note を確認。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。inprogress は 0 件。
- `00001KWZ5KERY` は close 済みで、merge commit `0334c572` と close commit `ffc16dea` が orchestration branch にある。

IntentPacket:

Intent:
- Runtime が Workspace filesystem を直接読まずに Backend-owned resource を取得できる Runtime-to-Backend resource fetch REST API を追加する。
- Backend-issued typed resource handle と Backend resource broker を導入し、v0 resource kind `profile_source_archive` を fetch / verify / cache できるようにする。
- 既存 Decodal ProfileSourceArchive implementation を、この resource fetch boundary に接続または将来接続しやすい形に整理する。

Binding decisions / invariants:
- Browser-facing API に Backend internal endpoint、resource credential、resource handle、raw path、Runtime endpoint/token/socket/session path を出さない。
- Runtime は Backend-issued resource handle なしに Workspace-derived resource を取得できない。
- handle には resource kind、workspace/scope id、resource id/digest、operation、expiry/nonce/revision/generation、redaction/max-bytes/content-type、audit correlation id を含める。
- handle には raw Backend URL、host absolute path、secret value、Browser request 由来 raw filesystem scope を含めない。
- v0 resource kind は `profile_source_archive`。Memory/Ticket/Objective tool backend 本体は非目標。
- Runtime-local filesystem discovery を増やさず、Backend authority / redaction / audit / capability 境界を保つ。
- WebSocket/persistent bidirectional channel は非目標。v0 は request/response REST/direct call。

Requirements / acceptance criteria:
- typed request/response schema の Runtime -> Backend resource fetch API がある。
- embedded Runtime direct call path と remote Runtime HTTP path が同じ resource contract を使う。
- Backend が resource handle を発行/検証し、`profile_source_archive` bytes を返せる。
- Runtime が resource handle から archive を fetch / digest verify / cache できる。
- expired / unauthorized / workspace-runtime-worker mismatch / missing resource / digest mismatch / oversized response は typed diagnostic。
- Browser-facing response redaction が tests で確認される。

Implementation latitude:
- exact endpoint/module/type names、cache layout、handle signing/nonce representation、audit record shape、profile archive integration depth は既存 Runtime/Workspace Server style に合わせてよい。
- 既存 `ConfigBundle` / `ProfileSourceArchive` machinery は維持しつつ、resource handle fetch boundary を追加する形でよい。
- v0 で remote Runtime full auth hardening が不足する場合は typed diagnostic と focused tests で境界を示す。

Escalate if:
- secret synchronization、Memory/Ticket/Objective tool implementations、persistent channel、Plugin package manager、multi-tenant auth、public Browser handle exposure、or Decodal archive format redesign が必要になる場合。

Validation:
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map:
- Decodal/Profile archive current implementation: `crates/worker-runtime/src/profile_archive.rs`, `config_bundle.rs`, `worker_backend.rs`, `runtime.rs`, `http_server.rs`, `catalog.rs`。
- Backend/Runtime bridge: `crates/workspace-server/src/hosts.rs`, `server.rs`, `companion.rs`。
- Protocol/runtime request shapes: `crates/protocol/src/lib.rs`, `crates/worker-runtime` request/response structs。

Critical risks / reviewer focus:
- resource handles must not become Browser-visible bearer authorities。
- endpoint/token/raw path/internal store/runtime details must stay out of Browser-facing surfaces。
- direct and remote Runtime paths must share contract rather than diverging ad hoc。
- digest/max-bytes/content-type validation must fail closed。
- profile archive fetch integration must not reintroduce Runtime workspace filesystem discovery。

---

<!-- event: state_changed author: orchestrator at: 2026-07-08T10:39:16Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after user follow-up 「06VAが進まない理由は何？ないなら順当に消化して」。

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0G06VA)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KX0G06VA)`: prior waiting note は `00001KWZ5KERY` inprogress/review によるもの。`00001KWZ5KERY` は now closed / merged。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。

Acceptance basis:
- previous waiting reason は解消済み。
- concrete missing decision / information は残っていない。
- Ticket の scope は Runtime-to-Backend resource fetch REST API v0 と `profile_source_archive` kind に閉じており、Memory/Ticket/Objective tool backend 本体、persistent channel、secret sync、plugin package manager は非目標として明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T10:40:25Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KX0G06VA-resource-fetch-api`
  - Branch: `work/00001KX0G06VA-resource-fetch-api`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KX0G06VA-resource-fetch` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---
