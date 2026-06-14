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

<!-- event: state_changed author: workspace-panel at: 2026-06-14T06:08:45Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-14T06:10:23Z -->

## Decision

Routing decision: queued_waiting_conflict

Reason:
- Ticket is queued and appears implementation-ready, but it touches Profile concrete scope / launch policy surfaces.
- `00001KTZY8HK2` was also queued and accepted in this routing pass for Profile API/resource migration (`extend` removal). Running both profile-surface migrations in parallel is likely to create merge conflicts and unclear review boundaries.
- No missing requirement or dependency blocker was identified; this is a conflict/migration-order wait.

Next action:
- Keep queued for now.
- Re-evaluate after `00001KTZY8HK2` is merged/validated, or if human explicitly authorizes parallel work despite conflict risk.

---
