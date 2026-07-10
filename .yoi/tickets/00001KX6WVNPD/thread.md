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
