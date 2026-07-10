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
