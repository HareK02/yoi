<!-- event: create author: "yoi ticket" at: 2026-07-10T21:16:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T21:23:27Z -->

## Decision

Worker 側が持つ workspace 情報は `Option<WorkspaceId>` に留める。`WorkspaceBackendRef`、endpoint、auth/SecretRef、adapter/client の materialization は Runtime/host 側の責務とし、Worker は Runtime から注入された scoped `WorkspaceClient` / handle 経由で workspace-aware API に request する。


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

<!-- event: decision author: hare at: 2026-07-10T22:00:16Z -->

## Decision

Clarification: this ticket should remove Worker-level `workspace_root: PathBuf`, not leave it as a staged authority surface. Local paths may remain only inside Runtime/host backend adapters or `WorkerFilesystemAuthority::Local`; Worker workspace identity is `Option<WorkspaceId>` and workspace operations go through an injected `WorkspaceClient` / handle.


---

<!-- event: intake_summary author: hare at: 2026-07-10T22:01:46Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:01:46Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T22:03:27Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T22:03:57Z -->

## Decision

Routing decision: blocked_by_dependency

Reason:
- Dashboard queue authorization was inspected, but this Ticket has an explicit typed `depends_on` relation to `00001KX6Y2A9Q`。
- `00001KX6Y2A9Q` is currently `inprogress` and is refactoring Worker filesystem authority / cwd removal / no-workdir boundaries。
- This Ticket replaces Worker `workspace_root` path usage with `WorkspaceBackend`, which builds on the same Worker struct/context/constructor boundary。
- Starting now would likely conflict with the active filesystem-authority refactor and obscure review boundaries。
- Therefore this routing pass leaves the Ticket `queued` and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / recent thread / artifacts。
- `TicketRelationQuery(00001KX6Y6ZEA)`: includes dependency on `00001KX6Y2A9Q`。
- `TicketList(inprogress)`: `00001KX6Y2A9Q` is the active inprogress Ticket。
- Orchestrator worktree status: clean。

Next action:
- Re-route this Ticket after `00001KX6Y2A9Q` is approved, merged, validated, and closed。
- Escalate only if a human explicitly requests combined implementation in the same branch/worktree。

---

<!-- event: decision author: orchestrator at: 2026-07-10T23:38:54Z -->

## Decision

Routing decision: hold_queued_due_migration_boundary_conflict

Reason:
- 前提 `00001KX6Y2A9Q` は closed になったため dependency 自体は解消済み。
- ただし、この routing pass では `00001KX6WVNPD` を先に受理した。
- `00001KX6WVNPD` は embedded no-workdir policy MVP であり、WorkspaceBackend migration を必要としない小さい境界変更である。
- この Ticket は Worker `workspace_root` path を `WorkspaceBackend` に置き換える refactor で、Worker struct/context/constructor/runtime spawn/UI API 周辺に触れる。
- `00001KX6WVNPD` も同じ Worker create / embedded runtime / no-workdir authority surface に触るため、並行 branch で進めると conflict と review boundary 混濁の可能性が高い。
- よって、この Ticket は `queued` のまま保持し、`00001KX6WVNPD` が reviewed/merged/closed になってから再 routing する。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- `TicketRelationQuery(00001KX6Y6ZEA)`: prerequisite `00001KX6Y2A9Q` は closed。
- `TicketList(inprogress)`: routing pass 中に `00001KX6WVNPD` を accepted for implementation。
- Orchestrator worktree status: clean。

Next action:
- `00001KX6WVNPD` の implementation/review/merge/close を完了後、この Ticket を再 routing する。
- 人間が combined branch を明示希望する場合のみ、この待機判断を再検討する。

---

<!-- event: decision author: orchestrator at: 2026-07-11T00:15:05Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 前提 `00001KX6Y2A9Q` は closed 済みで、`WorkerFilesystemAuthority::{None, Local}` と cwd 除去境界が merge 済み。
- 関連 `00001KX6WVNPD` も closed 済みで、embedded no-workdir Worker policy / UI/API optionality が merge 済み。
- この Ticket の typed `depends_on 00001KX6Y2A9Q` は解消済み。`00001KX6WVNPD` は `related` であり blocker ではなく、直前の conflict/migration-boundary waiting note もその Ticket の merge/close により解消済み。
- `TicketList(inprogress)` は 0 件で、Orchestrator worktree は clean。
- Ticket body/thread に Worker-level local path identity をなくし、Runtime/host-owned backend/client 境界へ移す binding decision が記録されている。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- Closed prerequisite `00001KX6Y2A9Q` and closed related policy Ticket `00001KX6WVNPD`。
- `TicketRelationQuery(00001KX6Y6ZEA)`: outgoing `depends_on 00001KX6Y2A9Q` is closed; other relations are `related` only。
- `TicketOrchestrationPlanQuery(00001KX6Y6ZEA)`: prior waiting notes were dependency/conflict notes now resolved by prerequisite and related Ticket closure。
- `TicketList(inprogress)`: 0 件。
- Orchestrator worktree status: clean。

IntentPacket:

Intent:
- Worker の workspace identity/context を local path (`workspace_root: PathBuf`) から path-free identity (`Option<WorkspaceId>`) と Runtime/host injected workspace client/handle 境界へ移行する。
- Filesystem authority は `WorkerFilesystemAuthority` に閉じ込め、workspace identity/client が local filesystem authority を意味しないようにする。

Binding decisions / invariants:
- Worker struct は durable/context identity として local workspace path を保持しない。
- Worker-level `workspace_root: PathBuf` field と local path-returning workspace identity accessor を削除する。
- Worker が `WorkspaceBackendRef` / endpoint / auth / SecretRef / adapter を直接解決しない。
- Runtime/host 側が backend binding source を保持・解決し、Worker には narrow `WorkspaceClient` / handle を注入する。
- `workspace_id = Some(...)` かつ `filesystem = None` を表現できること。
- `workspace_id = None` かつ `filesystem = None` の会話専用 Worker も表現できること。
- local path が必要な処理は Runtime/host backend adapter または `WorkerFilesystemAuthority::Local` の内側に閉じ込める。
- capability を導入する場合でも authority source にせず、tool registration / UX / discovery hint として扱い、enforcement は backend 側に置く。
- `WorkerFilesystemAuthority` / embedded no-workdir policy を崩さない。workspace root fallback や process cwd fallback を filesystem authority として復活させない。

Requirements / acceptance criteria:
- Worker context に `workspace_id: Option<WorkspaceId>` equivalent と injected workspace client/handle equivalent を持てる。
- Worker struct に `workspace_root: PathBuf` が存在しない。
- workspace-aware features（Ticket / memory / workflow / project records / workspace metadata）は local workspace path authority ではなく injected workspace client/handle 経由へ移行する、または移行が必要な箇所を fail-closed / unavailable diagnostic にする。
- Runtime/host が `WorkspaceBackendRef` equivalent を保持・解決し、Worker launch/restore/embedded/spawn path へ安全に materialize できる。
- No-workdir Worker は workspace API access と local filesystem authority の有無を独立に表現できる。
- Existing local/workdir Workers still work with `WorkerFilesystemAuthority::Local` and local-path-only operations confined there。
- Tests cover no local workspace path identity on Worker, workspace_id/client injection shape, no filesystem authority mixing, and representative workspace-aware API/tool behavior。

Implementation latitude:
- `WorkspaceId`, `WorkspaceBackendRef`, `WorkspaceClient` / handle の exact placement/name/API surface は existing crate boundaries に合わせてよい。
- Minimal client/handle can support only currently needed workspace-aware operations if broad API migration would exceed Ticket scope, but it must not leave Worker-local path authority as the active path。
- If a currently path-backed feature cannot be migrated safely in this Ticket, prefer explicit unavailable/fail-closed diagnostic with follow-up note rather than hidden path fallback。
- Capability discovery is optional unless needed for safe tool registration/UX。

Escalate if:
- Removing `workspace_root` from Worker requires changing product semantics for memory/Ticket/workflow availability beyond local refactor。
- A workspace-aware feature cannot be migrated without introducing a broad external backend/protocol design not captured by this Ticket。
- You need to reintroduce Worker-held local workspace path as identity/authority。
- You need to weaken no-workdir fs/Bash tool omission or `WorkerFilesystemAuthority` invariants。
- Public API/serialization migration requires incompatible durable-state handling beyond existing restore/test coverage。

Validation:
- `rg "workspace_root" crates/worker crates/worker-runtime crates/yoi-pod-client crates/yoi-workspace-server` with expected remaining hits only outside Worker identity/authority or documented adapter boundaries。
- `rg "worker\.workspace_root|workspace_root\(\)" crates` or equivalent showing Worker local path accessor removed。
- `git diff --check`
- `cargo test -p worker --lib --tests`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server --lib`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test` if API/types/UI are touched。
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map / likely touch points:
- `crates/worker/src/worker.rs` for Worker context fields and accessors。
- `crates/worker/src/controller.rs` for workspace-aware tool/resource setup currently tied to workspace path。
- `crates/worker-runtime/src/worker_backend.rs` for Runtime/embedded/restore construction and backend binding materialization。
- workspace-server/client crates if backend/client DTOs or Worker launch summaries expose workspace identity。
- tests around no-workdir Workers, restore, child spawn, Ticket/memory/workflow availability。

Critical risks / reviewer focus:
- Hidden local path authority retained in Worker under a renamed field/accessor。
- Workspace identity/client being treated as filesystem authority。
- WorkspaceBackendRef/endpoint/secret leaking into Worker-owned state。
- Breaking local/workdir Workers or child spawn/restore while removing path identity。
- Regressing embedded no-workdir policy or filesystem/Bash tool omission。
- Over-broad migration that invents a large remote-workspace protocol instead of the minimal safe boundary needed here。

---

<!-- event: state_changed author: orchestrator at: 2026-07-11T00:15:11Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Prerequisite and prior conflict/migration-boundary Ticket are now closed. This Ticket is accepted for implementation before creating a worktree or spawning role Pods.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-11T00:59:55Z -->

## Implementation report

Implementation progress report:

- Coder Pod `yoi-coder-00001KX6Y6ZEA-workspace-backend` completed implementation in `/home/hare/Projects/yoi/.worktree/00001KX6Y6ZEA-workspace-backend` on branch `work/00001KX6Y6ZEA-workspace-backend`。
- Implementation commit: `142b60e1b3ca refactor: separate worker workspace identity`。
- Orchestrator inspected worktree status, branch log, commit stats, `git diff --check 74b07bd7..HEAD`, and workspace-root accessor grep. Worktree was clean and diff check passed。

Implementation summary:
- Replaced Worker-held `workspace_root: PathBuf` identity with path-free `WorkerWorkspaceContext`, `WorkspaceId`, and `WorkspaceClient`。
- Kept local filesystem authority under `WorkerFilesystemAuthority::{None, Local(LocalWorkingDirectory)}`。
- Updated Worker constructors / restore / spawn paths to receive workspace context separately from filesystem authority。
- Added runtime-side `RuntimeWorkspaceBackendRef` as host-owned local backend binding that injects narrow Worker workspace context。
- No-filesystem Workers keep memory/workflow local layouts unavailable/fail-closed rather than falling back to workspace paths。
- Preserved legacy metadata `workspace_root` only as a local filesystem hint derived from `WorkerFilesystemAuthority::Local`, not as Worker identity。
- Updated tests for path-free workspace identity/client separation from local tool cwd/root, workspace client without filesystem authority, no local path-returning Worker workspace accessor, and constructor usage。

Files touched:
- `crates/worker/src/worker.rs`
- `crates/worker/src/controller.rs`
- `crates/worker/src/entrypoint.rs`
- `crates/worker/src/lib.rs`
- `crates/worker/src/discovery.rs`
- `crates/worker/src/ticket_event_notify.rs`
- `crates/worker/tests/*`
- `crates/worker-runtime/src/worker_backend.rs`
- `crates/worker-runtime/Cargo.toml`
- `crates/session-store/src/worker_metadata.rs`
- `Cargo.lock`
- `package.nix`

Coder-reported validation passed:
- `rg -n "workspace_root" crates/worker crates/worker-runtime crates/client crates/workspace-server` with remaining hits as legacy metadata/local filesystem adapter or host/runtime local backend boundaries plus tests。
- `rg -n "worker\\.workspace_root|pub fn workspace_root|fn workspace_root" crates/worker crates/worker-runtime crates/client crates/workspace-server || true`; no Worker `workspace_root()` accessor hits. Orchestrator grep observed an unrelated `crates/workspace-server/src/records.rs` project-record accessor hit outside Worker identity。
- `git diff --check`
- `cargo test -p worker --lib --tests`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server --lib`
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Not run by Coder:
- `cd web/workspace && deno task check && deno task test` because web/API/types were not touched。

Next action:
- Route to external Reviewer Pod before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-11T01:07:06Z status: approve -->

## Review: approve

External review result: approve

Evidence:
- Worker identity is path-free: `WorkerWorkspaceContext` carries `Option<WorkspaceId>` plus narrow `WorkspaceClient`/unavailable state, while `Worker` stores `workspace_context` and `filesystem_authority` instead of a `workspace_root: PathBuf` (`crates/worker/src/worker.rs:61-109`, `384-405`)。
- No `worker.workspace_root()` / local path-returning workspace accessor remains; local paths are exposed only through `WorkerFilesystemAuthority::Local` helpers such as `local_working_directory()` (`worker.rs:741-840`)。
- Runtime/host boundary now resolves local workspace backend context before constructing Workers (`crates/worker-runtime/src/worker_backend.rs:321-390`), and Worker construction accepts injected workspace context + filesystem authority (`worker.rs:4219-4260`)。
- Workspace/client without filesystem and no-workspace/no-filesystem Workers are represented and covered by tests (`worker.rs:5441-5530`; worker-runtime tests around `901+`)。
- Workspace-aware local features fail closed when no local filesystem authority is available; Ticket/memory/workflow/spawn-dependent surfaces require `filesystem_authority.local()` and otherwise return unavailable errors rather than cwd/root fallback (`crates/worker/src/controller.rs:630-672`, `709-779`)。
- Legacy `workspace_root` metadata is written only as a compatibility/local hint from local filesystem authority, not as Worker identity (`worker.rs:4604-4621`; `session-store/src/worker_metadata.rs`)。

Validation performed:
- `git diff --check 142b60e1^ 142b60e1`
- Focused grep for forbidden `workspace_root()` accessor: no matches。
- Focused tests:
  - `cargo test -p worker spawned_context_tests --lib`
  - `cargo test -p worker-runtime worker_spawn_receives_materialized_workspace_cwd_instead_of_source_repo --features ws-server,fs-store`
  - `cargo test -p worker-runtime adapter_dispatches_user_input_through_worker_run_lifecycle --features ws-server,fs-store`

Blockers: none。

Non-blocking notes:
- Remaining `workspace_root` hits are local runtime/entrypoint/spawn-tool adapter, legacy metadata, or test contexts; reviewer did not find Worker identity/accessor use。
- `set_active_with_workspace_context` preserves existing metadata fields when passed `None`; acceptable for this Ticket as compatibility metadata, but future no-workspace reuse of an existing Pod name may want explicit clearing semantics to avoid stale UI hints。

---
