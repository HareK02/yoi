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

<!-- event: decision author: orchestrator at: 2026-06-14T06:35:25Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The previous conflict/wait on `00001KTZY8HK2` is resolved: profile extend removal has been reviewed, merged, validated, and marked done.
- Ticket remains queued, has implementation-ready requirements/acceptance criteria, and relation records are non-blocking `related` links only.
- Risk flags are scope/delegation/profile/launch/restore, but binding invariants and acceptance tests are concrete.

Evidence checked:
- Ticket body/thread/relations/orchestration-plan records.
- Completed `00001KTZY8HK2` status and validation.
- Orchestrator worktree clean state.

IntentPacket:
- Move concrete filesystem `scope` / `delegation_scope` out of reusable Profiles and into launch policy/effective metadata construction. Preserve restore snapshot authority and explicit child delegation behavior.

Binding decisions / invariants:
- Profiles own reusable behavior/model/prompt/feature policy, not concrete runtime authority.
- Fresh Orchestrator launch effective authority: direct read original workspace; delegation read original workspace + write original workspace `.worktree`; no root workspace write delegation.
- Normal Companion/TUI launch still gets expected workspace write direct scope from launch policy.
- SpawnPod child direct scope is explicit delegated child scope, not inherited/profile scope.
- Restore preserves metadata snapshot and does not reapply current profile/default launch authority over saved scope.

Validation:
- focused scope/profile/client/pod/spawn/restore tests, `cargo build -p yoi`, `cargo fmt --check`, `git diff --check`; `nix build .#yoi` if packaging/resource changes warrant or feasible.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T06:35:44Z from: queued to: inprogress reason: orchestrator_acceptance_after_profile_conflict_resolved field: state -->

## State changed

Profile extend removal prerequisite/conflict has been resolved and merged. Ticket evidence, related records, orchestration plan, and clean workspace state were checked. No blockers remain; implementation can proceed in a dedicated child worktree.

---
