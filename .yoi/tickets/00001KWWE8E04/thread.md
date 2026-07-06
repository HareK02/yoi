<!-- event: create author: "yoi ticket" at: 2026-07-06T19:25:08Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-06T19:48:42Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-06T19:48:42Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-06T19:53:14Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-06T19:54:13Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Workspace Browser route/API を immutable workspace id で scope する具体的な URL/API contract、acceptance criteria、非目標を明記している。
- 1 Backend = 1 Workspace の現状でも request path の `workspace-id` と Backend current `workspace_id` を照合するという authority boundary が明確。
- Runtime `/v1/...` API とは混同しないこと、multi-workspace DB migration / slug / admin UI は非目標であることが明確。
- typed relation blocker は 0 件、OrchestrationPlan record は 0 件。
- queued notification は human authorized routing であり、今回の routing acceptance 後にのみ implementation side effect へ進める。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWWE8E04)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KWWE8E04)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。inprogress は 0 件。
- visible Pods: previous execution-workspace child Pods が idle で残っているが、`StopPod` 不使用方針のため停止せず、新規 unique Pod を使う。capacity blocker ではない。
- `TicketDoctor`: 0 errors / 既存 diagnostics のみ。
- Bounded code map: `crates/workspace-server/src/{identity.rs,server.rs,hosts.rs,config.rs,records.rs,repositories.rs}`, `web/workspace/src/routes/**`, `web/workspace/src/lib/workspace-sidebar/**`, workspace settings/sidebar/console helpers。

IntentPacket:

Intent:
- Workspace Browser の canonical UI route を `/w/<workspace-id>/...` に移し、Frontend が Browser-facing API を `/api/w/<workspace-id>/...` 経由で呼ぶようにする。
- 1 Backend = 1 Workspace の現状でも、path workspace id と serving workspace id を照合し、mismatch を typed sanitized diagnostic として fail closed する。

Binding decisions / invariants:
- Workspace URL は slug/renameable handle ではなく immutable `workspace_id` を使う。
- canonical route prefix は短い `/w/<workspace-id>/...`。
- Browser-facing scoped API prefix は `/api/w/<workspace-id>/...`。
- Runtime API `/v1/runtime`, `/v1/workers` はこの Ticket の scope 外であり、workspace id segment を追加しない。
- Backend canonical store の multi-workspace DB migration、Workspace slug/alias/rename、global admin UI は非目標。
- 既存 unscoped API/routes は互換 alias / redirect として残してよいが、Frontend の own calls/tests は scoped API/route を期待する。
- workspace id mismatch response は typed sanitized error とし、raw path / internal store / authority-bearing internals を漏らさない。

Requirements / acceptance criteria:
- `/w/<workspace-id>` が overview を表示する。
- `/w/<workspace-id>/repositories/<repository-id>` が Repository detail を表示する。
- `/w/<workspace-id>/objectives` と `/w/<workspace-id>/objectives/<objective-id>` が動く。
- `/w/<workspace-id>/settings` が Settings を表示する。
- `/w/<workspace-id>/runtimes/<runtime-id>/workers/<worker-id>/console` が Worker console を表示する。
- Frontend API calls use `/api/w/<workspace-id>/...` for workspace/repositories/objectives/workers/settings runtime-connections and relevant worker console endpoints。
- Sidebar/settings/repository/objectives/worker-console links include workspace id。
- mismatched workspace id returns typed 404 or equivalent mismatch diagnostic。
- Focused backend/web tests cover scoped API, mismatch, and frontend scoped API/link construction。

Implementation latitude:
- Route grouping/layout structure, helper naming, redirect vs compatibility route choice, and scoped API client abstraction can follow current SvelteKit/backend style。
- Existing unscoped API can remain as compatibility alias if scoped variants are canonical for frontend use。
- A small workspace route helper/client module is acceptable to avoid string duplication。
- Tests may cover helper/model/API routing rather than full browser E2E, consistent with current project constraints。

Escalate if:
- Multi-workspace DB/store authority must be implemented to satisfy routing。
- Workspace slug/handle/rename/alias becomes necessary。
- Runtime `/v1/...` API needs workspace id path segment。
- Browser-facing mismatch diagnostics require exposing raw internal paths/store/runtime details。

Validation:
- `cd web/workspace && deno task check && deno task test`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更がある場合だけ実行し、未実行なら理由を報告する。

Current code map:
- Workspace identity: `crates/workspace-server/src/identity.rs`, config/server setup in `config.rs`, `main.rs`。
- Backend routes/API: `crates/workspace-server/src/server.rs`, host/worker API context in `hosts.rs`, project records in `records.rs`, repository API in `repositories.rs`。
- Frontend routes: `web/workspace/src/routes/+layout.ts`, `+layout.svelte`, `+page.ts`, `+page.svelte`, `routes/repositories/[repositoryId]`, `routes/objectives`, `routes/settings`, `routes/runtimes/[runtimeId]/workers/[workerId]/console`。
- Frontend navigation/API call surfaces: `web/workspace/src/lib/workspace-sidebar/**`, settings model/page, worker console page, repository navigation helpers。

Critical risks / reviewer focus:
- Frontend must not keep calling unscoped `/api/...` for workspace-scoped data。
- Links must include the route `workspaceId`; no hardcoded current workspace routes should remain as canonical links。
- Scoped API mismatch must fail closed with sanitized typed diagnostics。
- Runtime `/v1/...` API must remain unchanged。
- Compatibility aliases must not become the frontend canonical path。
- workspace id should be treated as immutable id, not slug/alias。

---

<!-- event: state_changed author: orchestrator at: 2026-07-06T19:54:30Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWWE8E04)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWWE8E04)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。
- visible Pods / TicketDoctor / bounded code map を確認。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket の scope は Browser route/API workspace-id scoping に閉じており、multi-workspace DB migration / slug/alias / Runtime `/v1` scope change は非目標として明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-06T19:55:26Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KWWE8E04-workspace-id-routes`
  - Branch: `work/00001KWWE8E04-workspace-id-routes`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KWWE8E04-workspace-routes` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-06T20:27:15Z -->

## Implementation report

Implementation progress report:

- Coder Pod completed implementation and committed `f6ad9cfc feat: scope workspace routes by id` on branch `work/00001KWWE8E04-workspace-id-routes`.
- Worktree status was clean after commit.
- Changed files include backend `crates/workspace-server/src/server.rs` / `lib.rs`, scoped route/API helpers/tests, sidebar/repository/objective/settings/worker console links, and new SvelteKit route files under `web/workspace/src/routes/w/[workspaceId]/...`.
- Implementation summary:
  - Added canonical Browser routes under `/w/<workspace-id>/...`。
  - Added Browser-facing scoped API routes under `/api/w/<workspace-id>/...`。
  - Added backend scoped workspace id validation against current immutable backend workspace id。
  - mismatch returns typed sanitized `workspace_id_mismatch` diagnostic with 404。
  - Runtime `/v1/...` APIs were not changed。
  - Frontend links/API calls now use scoped route/API helpers for workspace Browser surfaces。
  - Worker creation console hrefs point at scoped `/w/<workspace-id>/runtimes/.../console`。
  - Focused backend and frontend tests cover scoped API/mismatch and route/API/link helpers。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（16 tests）
  - `cargo test -p yoi-workspace-server`: pass（65 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `yoi ticket doctor`: ok
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更ではないため未実行。
- External review will be requested via a read-only sibling Reviewer Pod. `StopPod` は使わない。

---
