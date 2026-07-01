<!-- event: create author: "yoi ticket" at: 2026-07-01T14:41:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-01T14:43:04Z -->

## Plan

Workspace Backend config file schema を定義する Ticket として詳細化した。

決定:
- 新規 Backend 設定項目は CLI flag として増やさない。
- config file は固定 path `<workspace_root>/.yoi/workspace-backend.local.toml` から読む。
- config file は workspace-local だが `.local` 名にして git 管理外にする。
- config は server/listen/static assets/data path override/limits/remote runtime source などの方針だけを持つ。
- data 本体、workspace identity、Ticket/Objective、Runtime fs-store、transcript、secret 値は config file に入れない。


---
