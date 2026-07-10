<!-- event: create author: "yoi ticket" at: 2026-07-10T21:13:49Z -->

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

<!-- event: decision author: hare at: 2026-07-10T21:46:21Z -->

## Decision

Clarification: `process_cwd` is not part of this design. The Worker-level `cwd: PathBuf` property and `worker.cwd()` accessor should be removed rather than kept as a legacy authority surface. Working directory exists only under `WorkerFilesystemAuthority::Local`; no-workdir uses `WorkerFilesystemAuthority::None`. Tool registration must be derived from that authority, not manifest override or cwd fallback.


---

<!-- event: intake_summary author: hare at: 2026-07-10T21:49:37Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T21:49:37Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T21:51:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T21:51:29Z -->

## Decision

Routing decision: accept for implementation

Evidence checked:
- Ticket `00001KX6Y2A9Q` is `queued` and has a concrete implementation scope。
- `TicketRelationQuery(00001KX6Y2A9Q)` shows incoming dependents (`00001KX6WVNPD`, `00001KX6Y6ZEA`) but no outgoing unresolved blockers for this Ticket。
- `TicketOrchestrationPlanQuery(00001KX6Y2A9Q)` had no prior plan; accepted plan was recorded in this routing pass。
- `TicketList(inprogress)` shows 0 inprogress Tickets。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` is clean。

Accepted plan:
- Create dedicated implementation worktree `/home/hare/Projects/yoi/.worktree/00001KX6Y2A9Q-worker-filesystem-authority` from current orchestration branch。
- Use branch `work/00001KX6Y2A9Q-worker-filesystem-authority`。
- Route implementation to sibling Coder Pod, then external review to sibling Reviewer Pod。
- Orchestrator retains merge, final validation, Ticket close, and child worktree/branch cleanup authority。

Implementation focus:
- Add explicit `WorkerFilesystemAuthority` with no-workdir `None` and local `Local(LocalWorkingDirectory)` representation。
- Remove Worker-level `cwd: PathBuf` field and `worker.cwd()` accessor。
- Gate core filesystem tools and Bash on `WorkerFilesystemAuthority::Local`; no-workdir Workers must not register those tools。
- Update constructor / restore / embedded / child-spawn paths to pass filesystem authority explicitly。
- Separate workspace context from filesystem authority; do not use workspace root / process cwd / runtime cwd fallback as no-workdir authority。
- Add tests for no-workdir model-visible tool surface and execution construction behavior, plus grep/equivalent assurance that `worker.cwd()` usages are gone。

---

<!-- event: state_changed author: orchestrator at: 2026-07-10T21:51:33Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Dashboard queue authorization was inspected, no blockers were found, and an accepted plan was recorded. Moving queued Ticket to inprogress before creating implementation worktree or spawning Pods.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T22:31:16Z -->

## Implementation report

Implementation progress report:

- Coder Pod `yoi-coder-00001KX6Y2A9Q-worker-fs-authority` completed implementation in `/home/hare/Projects/yoi/.worktree/00001KX6Y2A9Q-worker-filesystem-authority` on branch `work/00001KX6Y2A9Q-worker-filesystem-authority`。
- Implementation commit: `b50b94612a1185dc5bdc935d83d63f9d6ace826b feat: add explicit worker filesystem authority`。
- Orchestrator inspected the worktree status, branch log, commit stats, `git diff --check db3a7165..HEAD`, and grep checks for `worker.cwd()` / Worker `fn cwd`; worktree was clean, diff check passed, and no matches were found。

Coder-reported implementation summary:
- Added explicit `WorkerFilesystemAuthority::{None, Local(LocalWorkingDirectory)}` boundary。
- Removed Worker-level `cwd: PathBuf` and `worker.cwd()` accessor。
- Local Workers carry authority `root` and `cwd` separately。
- No-workdir embedded Worker spawn passes `WorkerFilesystemAuthority::None` explicitly instead of falling back to runtime/process cwd。
- Controller tool registration branches on local filesystem authority:
  - Local Workers register `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash` and get fs view/completions。
  - No-workdir Workers do not construct/register filesystem or Bash tools。
- Ticket / worker-spawn / peer-discovery cwd-dependent paths now require local filesystem authority instead of using workspace root as fallback。
- Tests added/updated for no-workdir tool omission, no cwd fallback, and local/materialized Worker filesystem/Bash behavior。

Coder-reported validation passed:
- `cargo test -p worker --lib --tests`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server --lib`
- `cargo check -p yoi`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link`
- `rg "worker\\.cwd\\(" crates || true`: no matches。
- `rg "pub fn cwd\\(" crates/worker/src/worker.rs || true`: no Worker cwd accessor remains。

Not run by Coder:
- `cd web/workspace && deno task check && deno task test` because web/API/types were not touched。

Next action:
- Route to external Reviewer Pod before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-10T22:40:35Z status: approve -->

## Review: approve

External review result: approve

Evidence:
- `WorkerFilesystemAuthority::{None, Local(LocalWorkingDirectory)}` and separate `root`/`cwd` are present in `crates/worker/src/worker.rs`; `Worker` now stores `filesystem_authority` plus separate `workspace_root`, with no `cwd: PathBuf` Worker field。
- Core filesystem/Bash registration is gated on `worker.local_working_directory()` in `crates/worker/src/controller.rs:619-636`; no-local Workers return `(None, None)` and do not construct/register `Read`/`Write`/`Edit`/`Glob`/`Grep`/`Bash`。
- Ticket and worker-spawn surfaces require local authority rather than falling back to `workspace_root` (`controller.rs:647-667`, `719-733`)。
- Embedded runtime creates `WorkerFilesystemAuthority::Local(binding.root, binding.cwd)` only when a working directory binding exists, otherwise `WorkerFilesystemAuthority::None` (`crates/worker-runtime/src/worker_backend.rs:297-310`)。
- Tests cover no-workdir omission and no cwd fallback via empty observed materialized cwds plus forbidden core tool names (`worker_backend.rs:1133-1201`), and materialized local Workers exposing core tools with materialized cwd (`worker_backend.rs:1203-1260`)。
- Grep check found no `worker.cwd()` / Worker `fn cwd`; the only remaining `pub fn cwd` hit is `WorkingDirectory` binding API, not `Worker`。

Blockers: none。

Validation performed by reviewer:
- Read-only ticket/diff/source/test inspection and grep checks。
- Reviewer did not rerun cargo/nix tests; coder-reported validation covers the requested test/build set。

Non-blocking note:
- Workspace-context surfaces such as memory/MCP/resident prompt loading can still be backed by `workspace_root` when enabled; this was treated as outside this Ticket’s focused core filesystem/Bash authority gate, but may need follow-up if the intended invariant becomes “no local file reads of any kind” for no-workdir Workers。

---
