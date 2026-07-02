<!-- event: create author: "yoi ticket" at: 2026-07-02T13:54:52Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T13:56:05Z -->

## Plan

Workspace Backend Runtime connection management と永続 config の Ticket として詳細化した。

方針:
- Runtime が Backend に接続するのではなく、Backend が Runtime source を RuntimeRegistry に登録する現行構造を前提にする。
- embedded Runtime は built-in connection として表示し、config 管理対象や削除対象にしない。
- remote Runtime connection は `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` に保存する。
- 管理 API は list/add/delete/test を提供する。
- v0 は config persistence を優先し、config 更新後は `restart_required = true` とする。live register/unregister は対象外。
- raw token 値、socket/session/store path、Runtime event cursor、live handle は UI/API/config に出さない。


---

<!-- event: decision author: hare at: 2026-07-02T14:00:49Z -->

## Decision

Runtime connection management Ticket は Settings shell 先行 Ticket に依存する形へ整理した。

Decision:
- `00001KWHJ0XH6` が Workspace-local Settings の entry point / route / shell / section navigation を用意する。
- `00001KWHHRTM9` はその Settings shell 上に Runtime Connections section と Backend API / config persistence を追加する。
- Runtime connection 管理 Ticket 内で Settings shell 設計を同時に進めない。


---
