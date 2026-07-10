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
