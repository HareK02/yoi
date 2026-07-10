<!-- event: create author: "yoi ticket" at: 2026-07-10T20:52:43Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-10T21:24:57Z -->

## Plan

整理結果:

1. 00001KX6Y2A9Q WorkerFilesystemAuthority
   - hard dependency なし。
   - no-workdir を filesystem authority none として型で表現する基盤。

2. 00001KX6WVNPD Embedded no-workdir Worker authority policy
   - depends_on: 00001KX6Y2A9Q。
   - UI/API の workdir 任意化と embedded no-workdir tool surface 制御を実装する。

3. 00001KX6Y6ZEA WorkspaceBackend / workspace_id 分離
   - depends_on: 00001KX6Y2A9Q。
   - workspace_root path 依存を workspace_id + Runtime-injected WorkspaceClient へ移行する。
   - embedded no-workdir MVP をブロックしない。

Hard dependencies は WorkerFilesystemAuthority を共通前提に限定する。WorkspaceBackend 分離は embedded no-workdir policy の MVP には related だが depends_on にはしない。


---

<!-- event: decision author: hare at: 2026-07-10T21:54:14Z -->

## Decision

前提チケット `00001KX6Y2A9Q` で Worker 直下の作業ディレクトリ property は削除されるため、このチケットはその存在を前提にしない。embedded workdir ありは `WorkerFilesystemAuthority::Local`、workdir なしは `WorkerFilesystemAuthority::None` として扱い、tool registration もその authority から導出する。


---

<!-- event: intake_summary author: hare at: 2026-07-10T21:59:20Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T21:59:20Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T22:00:07Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T22:00:33Z -->

## Decision

Routing decision: blocked_by_dependency

Reason:
- Dashboard queue authorization was inspected, but this Ticket has an explicit typed `depends_on` relation to `00001KX6Y2A9Q`。
- `00001KX6Y2A9Q` is currently `inprogress` and is implementing the Worker filesystem-authority / no-workdir / cwd-removal boundary this Ticket depends on。
- Starting this Ticket before that boundary is reviewed/merged would duplicate or conflict with constructor/restore/spawn/tool-registration changes。
- Therefore this routing pass leaves the Ticket `queued` and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / recent thread / artifacts。
- `TicketRelationQuery(00001KX6WVNPD)`: includes dependency on `00001KX6Y2A9Q`。
- `TicketList(inprogress)`: `00001KX6Y2A9Q` is the active inprogress Ticket。
- Orchestrator worktree status: clean。

Next action:
- Re-route this Ticket after `00001KX6Y2A9Q` is approved, merged, validated, and closed。
- Escalate only if a human explicitly requests combined implementation in the same branch/worktree。

---

<!-- event: decision author: orchestrator at: 2026-07-10T23:38:28Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 前提 Ticket `00001KX6Y2A9Q` は approved / merged / final-validated / closed 済みで、`WorkerFilesystemAuthority::{None, Local}` と no-workdir core filesystem/Bash tool omission boundary が利用可能になった。
- この Ticket は embedded Runtime の workdir 未指定 Worker 作成 policy と UI/API 制約に集中しており、目的・binding decision・受け入れ条件が具体的である。
- `TicketRelationQuery(00001KX6WVNPD)` の blocker view は空で、残る outgoing `depends_on 00001KX6Y2A9Q` は closed prerequisite になっている。
- `TicketOrchestrationPlanQuery(00001KX6WVNPD)` の既存 waiting note は prerequisite inprogress を理由にしたものだったため、現在は解消済み。
- Orchestrator worktree は clean。既存 child implementation branch/worktree は見当たらない。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- Related closed Ticket `00001KX6Y2A9Q` resolution/review/validation evidence。
- `TicketList(inprogress)`: 0 件。
- `git status --short --branch` and `git worktree list` in `/home/hare/Projects/yoi/.worktree/orchestration`。
- Current queue also contains related `00001KX6Y6ZEA`, but its Worker workspace-backend refactor touches overlapping Worker context/constructor surfaces; it will be held separately with a conflict/migration-boundary reason rather than started in parallel。

IntentPacket:

Intent:
- Embedded Runtime 選択時に、workdir 未指定の guidance/conversation-oriented Worker を作成できるようにする。
- Embedded no-workdir Worker は `WorkerFilesystemAuthority::None` として作られ、repository filesystem authority / cwd fallback / filesystem tools / Bash を持たない。

Binding decisions / invariants:
- no-workdir authority は merged `WorkerFilesystemAuthority::None` を使う。
- workspace root / process cwd / runtime cwd / manifest override / default scope を filesystem authority fallback にしない。
- filesystem tools (`Read`, `Write`, `Edit`, `Glob`, `Grep`) と `Bash` は no-workdir embedded Worker では construct/register/model-visible にしない。
- Bash は filesystem sandbox ではないため、no-workdir embedded では登録しない。
- non-embedded または local/filesystem-authority-required launch path では、既存の workdir 必須制約または明示 authority requirement を維持する。
- Worker 直下の `cwd` property / `worker.cwd()` 前提に戻さない。

Requirements / acceptance criteria:
- UI/API 経由で embedded Runtime の workdir 未指定 Worker create ができる。
- embedded + workdir ありは materialized binding を `WorkerFilesystemAuthority::Local` と Worker summary/detail に反映する。
- embedded + workdir なしは `WorkerFilesystemAuthority::None` になり、model-visible tool surface に fs tools / Bash が出ないことをテストで確認できる。
- `Glob` / `Grep` path 省略や `Bash` initial directory が no-workdir embedded の権限漏れにならない。
- Browser/UI では workdir optional が embedded Runtime に限られ、non-embedded/local authority path の制約を緩めない。

Implementation latitude:
- API DTO / UI form model / runtime spawn request shape の具体的な整理は coder が既存 pattern に沿って選んでよい。
- workdir 未指定状態の UI 表示文言・validation placement は、binding invariant を満たす範囲で local tactic としてよい。
- tests の配置は worker-runtime / workspace-server / web の既存近接 test pattern に合わせてよい。

Escalate if:
- embedded no-workdir Worker に workspace API access を付けるために `WorkspaceBackend` / `WorkspaceClient` の先行導入が必須になる場合。
- no-workdir Worker へ fs/Bash tool の error-only stub や empty-scope fallback を登録したくなる場合。
- non-embedded Worker の workdir requirement を緩める必要が出る場合。
- `WorkerFilesystemAuthority` の既存 invariant を変更する必要が出る場合。

Validation:
- `git diff --check`
- focused grep/check that no no-workdir path reintroduces `worker.cwd()` or local cwd fallback。
- `cargo test -p worker --lib --tests` if worker crate is touched。
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server --lib`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test` if web/API/types are touched。
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map / likely touch points:
- `crates/worker/src/worker.rs`, `crates/worker/src/controller.rs` for existing authority boundary verification only if needed。
- `crates/worker-runtime/src/worker_backend.rs` / working-directory binding and embedded spawn paths。
- `crates/workspace-server/src/server.rs` / API DTOs for Worker create/runtime options。
- `web/workspace` worker creation UI/types if workdir optionality is exposed there。

Critical risks / reviewer focus:
- Hidden filesystem authority through workspace root / process cwd / runtime cwd fallback。
- no-workdir Worker showing fs tools/Bash in model-visible schema。
- empty-scope/error-only fs tool registration rather than no registration。
- UI/API accidentally allowing non-embedded no-workdir Worker where local filesystem authority is still required。
- regression of embedded workdir-present Worker status/tool cwd behavior。

---

<!-- event: state_changed author: orchestrator at: 2026-07-10T23:38:34Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Prerequisite `00001KX6Y2A9Q` is now closed and merged. This Ticket is accepted for implementation before creating a worktree or spawning role Pods.

---
