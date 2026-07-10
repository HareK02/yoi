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
