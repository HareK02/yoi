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

<!-- event: implementation_report author: orchestrator at: 2026-07-08T11:20:23Z -->

## Implementation report

Implementation progress report:

- Coder Pod completed implementation and committed `57e96d3b feat: add backend resource fetch api` on branch `work/00001KX0G06VA-resource-fetch-api`.
- Worktree status was clean after commit.
- Changed files include:
  - `crates/worker-runtime/src/resource.rs` (new)
  - `crates/workspace-server/src/resource_broker.rs` (new)
  - `crates/worker-runtime/src/{config_bundle.rs,http_server.rs,main.rs,profile_archive.rs,worker_backend.rs}`
  - `crates/workspace-server/src/{hosts.rs,server.rs,companion.rs}`
  - `Cargo.lock`, `package.nix`
- Implementation summary:
  - Added typed resource handle/request/response model and Runtime resource client/cache path。
  - Added Backend resource broker for v0 `profile_source_archive` resources。
  - Embedded direct path and remote Runtime HTTP path share the resource contract。
  - Runtime verifies digest, max bytes/content type, expiry/binding, and cache reuse before using fetched profile archives。
  - Config bundle/profile archive path can use resource handle without Runtime filesystem discovery。
  - Browser-facing summaries redact internal resource handles/endpoints/raw paths/archive details。
  - Added focused tests for handle issue/verify, runtime mismatch, digest mismatch, oversized response, direct/remote resource fetch, Browser redaction, and config-bundle/resource archive integration。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（42 lib tests + 5 main tests + doc tests）
  - `cargo test -p yoi-workspace-server`: pass（71 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（17 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass
- External review will be requested via a read-only sibling Reviewer Pod. `StopPod` は使わない。

---

<!-- event: review author: reviewer at: 2026-07-08T11:27:23Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blockers:

1. Backend handle verification が Runtime から送られた mutable handle fields を信頼している。
   - `crates/workspace-server/src/resource_broker.rs` の fetch validation は expiry / max-bytes を `request.handle` から確認しており、broker-stored issued handle の authority fields と照合していない。
   - 有効な nonce/resource/digest を持つ Runtime が以下を改変できる可能性がある。
     - `expires_at_unix_seconds` を延長して expiry を bypass。
     - `max_bytes` を増やして oversized rejection を bypass。
     - redaction/scope/generation など他 policy fields も stored issued handle に bind されていない。
   - Backend-issued handle を Backend が検証し、expired/oversized handles が fail closed typed diagnostics になるという Ticket requirement に反する。

2. Runtime cache が Backend authorization / expiry / binding を bypass している。
   - `ProfileRuntimeWorkerFactory::resolve_profile_source_archive` は fetch 前に digest keyed `profile_archive_cache` を見る。
   - 一度 archive が cache されると、同一 digest の後続 config bundle は Backend verification なしで reuse でき、expired handle / runtime-worker-workspace mismatch でも成功し得る。
   - 「Runtime cannot fetch/use Workspace-derived resources without Backend-issued handle」「Runtime verifies expiry/binding」「expired/unauthorized/mismatch cases produce diagnostics」に反する。

3. security contract の acceptance-level tests が不足している。
   - happy-path broker issue/fetch、runtime mismatch、browser projection redaction はあるが、以下の meaningful coverage が不足している。
     - tampered expired handle / expiry extension。
     - worker mismatch。
     - REST 経由 missing resource。
     - Runtime response verification の digest mismatch。
     - tampered `max_bytes` による oversized response。
     - remote HTTP resource-fetch path が同じ contract を使うこと。
   - 上記 defect が存在しているため、単なる coverage gap ではなく core Ticket intent の boundary regression を許している。

Validation / inspection performed:
- implementation worktree `/home/hare/Projects/yoi/.worktree/00001KX0G06VA-resource-fetch-api` のみを使用。
- Ticket item/thread/orchestration plan を確認。
- `git show --stat --check 57e96d3b`, `git show --name-only 57e96d3b`, relevant implementation files を inspection。
- Orchestrator-reported validation pass を参照し、Reviewer は heavy tests を再実行していない。

Non-blocking follow-ups:
- New `reqwest` dependency and `package.nix` cargo hash update は HTTP Runtime resource client と `nix build` pass により妥当。
- broader resource kind 追加前に、stored issued handle policy fields の完全比較、または signed/opaque handle representation を検討するとよい。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T11:45:42Z -->

## Implementation report

Review-fix implementation report:

- Coder Pod completed requested changes and committed follow-up `e716ae44 fix: harden backend resource handles` on branch `work/00001KX0G06VA-resource-fetch-api`.
- Worktree status was clean after commit.
- Fix summary:
  - Backend broker now treats broker-stored issued handle as authority。
  - Request handle must match stored issued handle; expiry and `max_bytes` validation use stored policy fields, not mutable Runtime-provided fields。
  - Tampered scope/policy fields fail closed。
  - Runtime archive cache no longer bypasses Backend authorization: Runtime calls `BackendResourceClient` and verifies response before reusing digest-cached archive。
  - Expired/unauthorized handles now block cached reuse。
  - Added focused security-contract tests for expiry extension/expired handle rejection, worker mismatch, policy tampering, REST missing-resource diagnostic, Runtime digest mismatch, oversized response despite `max_bytes` tampering, and remote HTTP path using the same Backend contract as direct broker path。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（45 lib tests + 5 main tests + doc tests）
  - `cargo test -p yoi-workspace-server`: pass（77 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（17 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass
- Requesting follow-up external review against `57e96d3b..e716ae44` and full combined implementation。

---

<!-- event: review author: reviewer at: 2026-07-08T11:53:02Z status: approve -->

## Review: approve

External follow-up review result: approve

Blockers: none.

Validation performed:
- Read-only review in implementation worktree `/home/hare/Projects/yoi/.worktree/00001KX0G06VA-resource-fetch-api`。
- Ticket item/thread, branch/log/status, `57e96d3b..e716ae44` fix diff, and relevant implementation files を確認。
- Reviewer は validation suite を再実行せず、Orchestrator-reported passing validation を evidence として確認。

Findings:
- Prior blocker 1 fixed: Backend は mutable request handle policy fields を信頼しない。`BackendResourceBroker` は issued `BackendResourceHandle` を resource と共に保存し、stored expiry/max_bytes を検証してから request handle と stored issued handle を比較する。tampering は `Expired` / `Oversized` / `Unauthorized` diagnostics で fail closed。expiry extension、policy tampering、worker/runtime mismatch、max_bytes tampering tests あり。
- Prior blocker 2 fixed: Runtime cache は Backend authorization を bypass しない。`resolve_profile_source_archive` は先に Backend fetch を行い、handle に対して response verify してから digest cache を reuse/insert する。cache test は second-use backend `Expired` rejection と backend call count を確認している。
- Prior blocker 3 fixed: security contract tests が meaningful に追加されている。
  - `resource_broker.rs`: expired/expiry extension、worker mismatch、runtime mismatch、policy tampering、stored max_bytes oversized。
  - `resource.rs`: Runtime digest mismatch と oversized response verification。
  - `server.rs`: REST missing-resource diagnostic と remote HTTP contract/tampering path。
  - `worker_backend.rs`: cache authorization。
  - Browser redaction: `ConfigBundleSummary` redaction test。
- Typed/direct/remote contract: `BackendResourceFetchRequest/Response/Error` は shared typed structs。embedded direct path は `BackendResourceBroker` を `BackendResourceClient` として使い、remote HTTP は同じ request shape を `/internal/runtime/resources/fetch` に POST する。
- Browser-facing redaction remains strong: browser-safe summaries は profile source graph summary のみを出し、handle nonce/resource id/archive digest/runtime-worker binding/token/internal endpoint/raw path を出さない。
- ProfileSourceArchive integration は normal Backend/Browser launch で Backend-owned resource boundary を使う。generated bundles は resource handle を持ち inline archive を持たず、Runtime は fetched archive bytes から resolve する。filesystem profile discovery は explicit no-config-bundle compatibility/debug fallback のみ。

Non-blocking follow-ups:
- stored `audit_correlation_id` の echo と Runtime-side response `resource_id` check を追加すると diagnostic integrity がさらに強くなる。
- remote HTTP resource fetch が trusted Backend/Runtime links を越えて露出する場合は、JSON body materialization 前の explicit client-side timeout/response-size limiting を検討する。

Effect on queued `00001KX0DSMPT`:
- blocking concern なし。profile/archive details は redacted summaries behind boundary に留まっており、future settings UI/API が profile metadata を consume しても handles を露出しない設計と互換。

---

<!-- event: state_changed author: orchestrator at: 2026-07-08T11:54:39Z from: inprogress to: done reason: merged_validated_review_approved field: state -->

## State changed

Merge/validation acceptance:

- Follow-up external review approved the implementation after blocker fix。
- Implementation branch `work/00001KX0G06VA-resource-fetch-api` was merged into the orchestration branch with merge commit `01f94b75 merge: backend resource fetch api`。
- Final validation in Orchestrator worktree passed:
  - `git diff --check`: pass
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（45 lib tests + 5 main tests + doc tests）
  - `cargo test -p yoi-workspace-server`: pass（77 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（17 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass

Implementation commits:
- `57e96d3b feat: add backend resource fetch api`
- `e716ae44 fix: harden backend resource handles`

No merge-blocking issues remain. Reviewer noted no blocking concern for queued `00001KX0DSMPT`; the resource-fetch API shape is now stable enough for that follow-up to be re-routed.

---

<!-- event: state_changed author: hare at: 2026-07-08T11:54:54Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-08T11:54:55Z status: closed -->

## 完了

完了。

実装内容:
- Runtime-to-Backend resource fetch contract を追加した。
- Backend-issued typed resource handle と `BackendResourceBroker` を実装した。
- v0 resource kind `profile_source_archive` を fetch / verify / cache できるようにした。
- embedded direct Runtime path と remote Runtime HTTP path が同じ typed resource contract を使うようにした。
- Runtime は handle に基づき archive を fetch し、digest / max bytes / content type / expiry / binding を検証してから cache/reuse するようにした。
- Backend broker は stored issued handle を authority として扱い、Runtime-provided mutable handle fields による expiry / max_bytes / policy tampering を fail closed するようにした。
- Runtime cache は Backend authorization / expiry / binding を bypass しないようにした。
- Decodal ProfileSourceArchive / config bundle path を resource fetch boundary に接続し、Runtime filesystem discovery を再導入しない形にした。
- Browser-facing summaries は internal resource handle / endpoint / raw path / archive content/digest/store path / token 等を出さないようにした。
- Focused worker-runtime / workspace-server tests を追加した。

主な commit / merge:
- implementation: `57e96d3b feat: add backend resource fetch api`
- review fix: `e716ae44 fix: harden backend resource handles`
- merge into orchestration: `01f94b75 merge: backend resource fetch api`

Review:
- 初回 external review は request_changes。
- follow-up external review は approve。merge-blocking issue なし。

Final validation in Orchestrator worktree:
- `git diff --check`: pass
- `cargo test -p worker-runtime --features ws-server,fs-store`: pass（45 lib tests + 5 main tests + doc tests）
- `cargo test -p yoi-workspace-server`: pass（77 lib tests + 2 main tests）
- `cargo check -p yoi`: pass
- `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `cd web/workspace && deno task test`: pass（17 tests）
- `yoi ticket doctor`: ok
- `nix build .#yoi --no-link`: pass

補足:
- Reviewer の non-blocking follow-up: stored `audit_correlation_id` echo と Runtime-side response `resource_id` check、remote HTTP response-size limiting のさらなる強化余地。
- queued `00001KX0DSMPT` については、resource-fetch API shape が確定したため再 routing 可能。

---
