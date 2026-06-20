<!-- event: create author: "yoi ticket" at: 2026-06-20T10:48:57Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-20T10:49:26Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-20T10:49:26Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T11:52:33Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T11:53:35Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body は Profile launch 時に workspace override 由来の追加 `scope.allow` が `apply_profile_launch_policy()` の `workspace_scope(...)` 再代入で失われる具体原因、再現例、維持すべき既定 scope / delegation、Ticket role policy、受け入れ条件を実装可能な粒度で定義している。
- 未解決 relation blocker はない。
- 現在 queued はこの Ticket のみ、inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は scope / profile / launch-policy / security boundary だが、Ticket は workspace root write scope と `.worktree` write deny の維持、Ticket role launch constraints、snapshot と tool-visible scope の一致、restore non-goal を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVJABS1A` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVJABS1A)`: no blockers。
- `TicketOrchestrationPlanQuery(00001KVJABS1A)`: no previous plan records; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `9e7c84a4`。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching implementation branch/worktree。

IntentPacket:

Intent:
- Fix Profile launch policy so explicit additional `scope.allow` entries from Profile / workspace override survive the final launch policy application。
- Preserve the safe workspace defaults and role-specific constraints while ensuring `resolved_manifest_snapshot.scope.allow` matches the actual readable/writable tool scope presented to the Pod。

Binding decisions / invariants:
- Do not discard explicit Profile/override `scope.allow` entries when adding workspace default scope。
- Preserve normal Pod launch default workspace root write scope。
- Preserve `.worktree` write deny default behavior。
- Preserve Ticket role launch constraints and delegation defaults。
- Do not re-evaluate overrides during restore from existing metadata snapshot; restore behavior is out of scope unless tests reveal an accidental regression。
- Snapshot saved in Pod metadata must reflect final effective manifest/scope, not an intermediate manifest。
- Avoid broad profile/config semantics changes beyond launch policy scope merging。

Requirements / acceptance criteria:
- Test that `.yoi/override.local.toml` extra `[[scope.allow]]` remains in `resolved_manifest_snapshot.scope.allow` after Profile launch。
- Test that normal Pod launch still receives workspace root write scope and `.worktree` write deny。
- Test that Ticket role launch scope/delegation defaults are not broken。
- Relevant `cargo test` / `cargo check` / `cargo fmt --check` / `git diff --check` pass。

Escalate if:
- Fixing the merge would broaden runtime authority beyond explicit profile/override scope。
- Current scope model cannot distinguish launch-policy default grants from user-specified grants without a schema/API decision。
- Ticket role policy requires an authority decision not specified in the Ticket。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T11:53:45Z from: queued to: inprogress reason: orchestrator_acceptance_profile_override_scope field: state -->

## State changed

Ticket body/thread, relation metadata, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded Profile launch/scope policy context were checked. There is no unresolved blocking dependency, no inprogress/capacity blocker, and no missing planning decision. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T11:54:59Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `72e9f2f1 ticket: accept profile override scope launch`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVJABS1A-profile-override-scope` on branch `impl/00001KVJABS1A-profile-override-scope` at `72e9f2f1`。
- Spawned Coder Pod `yoi-coder-00001KVJABS1A` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, broad profile/config semantic changes, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T12:06:19Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVJABS1A`.

Implementation commit:
- `0717aae3 pod: preserve profile override scope`

Changed areas reported:
- `crates/pod/src/entrypoint.rs`:
  - Profile launch policy no longer replaces `manifest.scope` wholesale。
  - It appends missing launch-policy default scope rules onto the already-resolved Profile/override scope。
  - Explicit `scope.allow` / `scope.deny` entries from Profile and `.yoi/override.local.toml` are preserved。
  - Normal workspace write scope and `.worktree` write deny remain applied for normal launches。
  - Ticket role launch defaults/delegation behavior remains applied。
  - Added focused tests for:
    - `.yoi/override.local.toml` extra `[[scope.allow]]` surviving Profile launch in final manifest/snapshot serialization。
    - Normal launch keeping workspace write + `.worktree` deny while preserving explicit Profile scope。
    - Orchestrator role launch keeping read-root/worktree delegation defaults while preserving explicit Profile scope。

Coder validation reported:
- `cargo test -p pod entrypoint::tests::`: passed, 22 tests。
- `cargo check -p pod`: passed。
- `cargo fmt --all --check`: passed。
- `git diff --check`: passed。
- `cargo test -p pod`: ran but failed on two existing prompt guidance assertions unrelated to this change:
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
  - Missing text asserted: `"worktree status, diff, and test results"`。
- Nix not run because no packaging/source-filter/dependency files changed。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `0717aae3`。
- Diff from acceptance `72e9f2f1..HEAD` is one implementation commit touching only `crates/pod/src/entrypoint.rs`, about 111 insertions / 14 deletions。
- `git diff --check 72e9f2f1..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on scope allow/deny merge semantics, authority boundary/no broadening beyond explicit override + defaults, workspace write / `.worktree` deny preservation, Ticket role policy preservation, snapshot/tool-visible scope consistency, and whether the full `cargo test -p pod` failure is unrelated/pre-existing。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T12:07:00Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVJABS1A-r1` against implementation branch `impl/00001KVJABS1A-profile-override-scope`。
- Review target commit: `0717aae3 pod: preserve profile override scope`。
- Review baseline: `72e9f2f1`。
- Reviewer task focuses on scope allow/deny merge semantics, no authority broadening beyond explicit override + launch defaults, workspace write / `.worktree` deny preservation, Ticket role launch/delegation constraints, metadata snapshot/effective scope consistency, restore non-goal, and reported full `cargo test -p pod` failure triage。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVJABS1A-r1 at: 2026-06-20T12:12:10Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket contract / Orchestrator IntentPacket。
- Implementation diff: `72e9f2f1..0717aae3`。
- 変更ファイル: `crates/pod/src/entrypoint.rs` のみ。
- Launch-policy change、snapshot/tool-visible scope path、restore path、focused tests を確認。

Approval evidence:
- `append_missing_rules` / `apply_scope_launch_defaults` により、launch-policy defaults は既に解決済みの Profile / override scope を置換せず、missing rules として append される。
- Normal launch は workspace-root write scope と `.worktree` write deny を引き続き append する。
- Ticket role launch は role-specific default direct scope と delegation defaults を引き続き適用する。
- `resolve_manifest()` は `apply_profile_launch_policy()` 後の final manifest を返す。
- `Pod::from_manifest_with_context` は `manifest.scope` から tool-visible scope を作る。
- Pod metadata snapshot serialization は final manifest を使う。
- Restore path は existing `resolved_manifest_snapshot` がある場合それを使うため、この変更で restore 時に override を再評価する挙動は入っていない。
- Focused tests は override-local `scope.allow` survival、normal profile launch defaults、Orchestrator role default scope/delegation preservation を cover している。

Blocking issues: none。

Non-blocking concerns / follow-ups:
- Full `cargo test -p pod` は以下 2 件の prompt-guidance assertion failure で失敗する。
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
  - Missing asserted text: `"worktree status, diff, and test results"`
- Reviewer判断: この branch diff は `crates/pod/src/entrypoint.rs` のみであり、prompt rendering/assertion paths / prompt resources / catalog tests を変更していないため、この failure は unrelated/pre-existing。

Reviewer validation:
- `cargo fmt --all --check`: passed。
- `git diff --check 72e9f2f1..HEAD`: passed。
- `cargo test -p pod entrypoint::tests::`: passed, 22 tests。
- `cargo check -p pod`: passed。
- `cargo test -p pod`: unrelated prompt assertion failures only; 410 passed, 2 failed。

Worktree status at review end: clean。

---
