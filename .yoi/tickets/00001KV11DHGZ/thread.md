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

<!-- event: implementation_report author: hare at: 2026-06-14T06:53:30Z -->

## Implementation report

実装報告（Coder）

Commit:
- implementation: `21bf009a3f95978007468005982903c8d7cae9e7` (`feat: move profile scope to launch policy`)

変更ファイル:
- `resources/profiles/default.lua`
- `resources/profiles/companion.lua`
- `resources/profiles/intake.lua`
- `resources/profiles/orchestrator.lua`
- `resources/profiles/coder.lua`
- `resources/profiles/reviewer.lua`
- `crates/manifest/src/profile.rs`
- `crates/manifest/src/config.rs`
- `crates/pod/src/entrypoint.rs`
- `crates/pod/src/spawn/tool.rs`

実装内容:
- Builtin reusable Profiles から concrete filesystem `scope` / `delegation_scope` を削除した。
- Profile resolution の implicit default workspace-write scope を廃止し、Profiles without scope を empty scope として解決可能にした。
- Fresh profile launch の effective authority を `pod` entrypoint の launch policy で付与するようにした。
  - normal Companion/TUI/default profile launch: workspace write direct scope（`.worktree` write deny）+ workspace read / `.worktree` write delegation。
  - Ticket Orchestrator role launch: original workspace read direct scope + original workspace read / `<workspace>/.worktree` write delegation。root workspace write delegation は付与しない。
  - Ticket Intake/Reviewer role launch: workspace read direct scope。
  - Ticket Coder role launch: workspace write direct scope。
- Single-file `--manifest` mode は従来どおり concrete `scope.allow` を要求し、Profile launch policy と混同しないようにした。
- SpawnPod child scope replacement path は維持し、narrow Orchestrator delegation で Coder/Reviewer 用の root-read + implementation-worktree-write validation が成立することをテストした。
- Existing user Profile `scope` / `delegation_scope` compatibility は deprecated-compatible path として残した（Ticket の選択肢に沿って、builtin role launch authority は Profile scope に依存しない）。

Validation:
- `cargo test -p manifest` ✅
- Focused tests ✅
  - `cargo test -p manifest builtin_role_profiles_are_registered_and_resolve --quiet`
  - `cargo test -p manifest builtin_default_resolves_without_external_evaluator --quiet`
  - `cargo test -p manifest resolve_accepts_empty_scope_for_profile_launch_policy --quiet`
  - `cargo test -p pod --lib entrypoint::tests::normal_startup_uses_default_profile --quiet`
  - `cargo test -p pod --lib entrypoint::tests::orchestrator_profile_launch_gets_read_root_and_worktree_delegation_from_launch_policy --quiet`
  - `cargo test -p pod --lib spawn::tool::tests::orchestration_delegation_allows_root_read_and_worktree_writes_not_root_writes --quiet`
  - `cargo test -p pod --lib pod::pod_metadata_restore_manifest_tests::snapshot_preserves_saved_scope_over_current_manifest --quiet`
  - `cargo test -p client ticket_role --quiet`
- `cargo build -p yoi` ✅
- `cargo fmt --check` ✅
- `git diff --check` ✅
- `nix build .#yoi` not run: no Cargo.lock, packaging, or resource inclusion pattern changed.

Residual risks / notes:
- Full `cargo test -p pod --lib` was attempted and still has two prompt-text assertion failures (`worktree status, diff, and test results`) in prompt catalog/system tests; this diff did not touch prompt resources or those assertions. Focused scope/profile/spawn/restore tests passed.
- User Profile `scope` compatibility remains supported for now; future schema cleanup can remove or deprecate it explicitly if desired.


---
