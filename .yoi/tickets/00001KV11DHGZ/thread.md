<!-- event: create author: "yoi ticket" at: 2026-06-13T17:45:32Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-13T19:02:42Z -->

## Decision

決定:
- 旧方針の「Orchestrator delegation scope だけを狭める」ではなく、1 Ticket にまとめて「Profile から concrete scope を外し、launch policy が runtime authority を付与する」方針に広げる。
- Profile は reusable behavior / prompt / model / feature policy を持つ層とし、filesystem `scope` / `delegation_scope` は起動経路が concrete workspace/cwd とともに決める。
- Orchestrator の desired effective authority は launch policy で `direct read workspace` + `delegation read workspace, write workspace/.worktree` として構築する。
- Lua/profile replacement API (`00001KTZY8HK2`) はこの scope 問題の前提にしない。scope 以外の replacement が必要なら後続として扱う。


---
