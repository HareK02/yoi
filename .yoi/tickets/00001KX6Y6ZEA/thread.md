<!-- event: create author: "yoi ticket" at: 2026-07-10T21:16:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T21:23:27Z -->

## Decision

Worker 側が持つ workspace 情報は `Option<WorkspaceId>` に留める。`WorkspaceBackendRef`、endpoint、auth/SecretRef、adapter/client の materialization は Runtime/host 側の責務とし、Worker は Runtime から注入された scoped `WorkspaceClient` / handle 経由で workspace-aware API に request する。


---
