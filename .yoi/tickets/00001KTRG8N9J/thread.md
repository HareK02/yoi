<!-- event: create author: "yoi ticket" at: 2026-06-10T10:11:51Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-10T10:15:27Z -->

## Plan

## Intake refinement

Readiness: `implementation_ready`。

既存 Ticket `00001KTRG8N9J` の body/thread/artifacts を確認した。artifacts は `.gitkeep` のみで、thread は作成イベントのみ。新規 duplicate Ticket は作成しない。

関連確認:
- closed `00001KTR6D3C5`: Lua Profile の global `yoi` API と `yoi.profile.import/extend` は実装済み。この Ticket はその follow-up として成立している。
- closed `00001KTR6YVDB`: LLM-facing Ticket role launch prompt prose は `resources/prompts` 側へ移行済み。Profile に prompt / workflow 文言を埋め込まない非目標と整合している。
- closed `00001KTNQK1V8`: role profile の feature/tool policy は明示 feature flags として整理済み。現在の `.yoi/profiles/*.lua` から builtin role profiles へ移す対象が明確。
- closed `00001KTG16J8S` / `00001KTG16J8R`: Ticket role launch config は明示 concrete profile selector を要求する方針で、`.yoi/ticket.config.toml` の `project:*` selector を `builtin:*` selector へ移行する要件と整合している。

現在の workspace 状態として、`.yoi/ticket.config.toml` は `project:intake` / `project:orchestrator` / `project:coder` / `project:reviewer` を参照し、`.yoi/profiles.toml` と `.yoi/profiles/*.lua` が role profiles を定義している。`resources/profiles/default.lua` は global `yoi` style で、builtin role profiles の base として使える前提がある。

Blocking open questions: なし。

Implementation latitude:
- `.yoi/profiles.toml` / `.yoi/profiles/*.lua` を削除するか、builtin override sample として残すかは実装時に判断してよい。ただし残す場合は project override としての意味を明確にし、実装報告に理由を残す。
- exact な test 配置は実装時に選んでよいが、builtin registry/resolver と Ticket role launcher/config の回帰を両方カバーすること。

Risk flags / reviewer focus: `profile-manifest`, `runtime-resource`, `authority-boundary`, `tool-surface`, `role-launch-config`, `prompt-boundary`, `packaging-resource`。

Escalation conditions:
- builtin role profile に resolved Manifest、runtime-bound field、concrete delegated scope/path、Pod/session 固有情報を入れる必要が出た場合。
- role workflow / LLM-facing prompt 文言を Profile へ移す必要が出た場合。
- `project:*` override semantics や existing registry selector semantics を壊す必要が出た場合。
- role profiles の tool/feature policy を現行より広く grant する必要が出た場合。

Validation focus:
- `builtin:companion` / `builtin:intake` / `builtin:orchestrator` / `builtin:coder` / `builtin:reviewer` が resolver で解決できること。
- `.yoi/ticket.config.toml` の role selectors が builtin selectors になり、Ticket role launcher が起動計画を作れること。
- manifest crate の profile tests と、必要なら client Ticket role tests を更新すること。
- runtime resource / packaging に触れるため、完了前に `nix build .#yoi` まで確認すること。

---

<!-- event: intake_summary author: intake at: 2026-06-10T10:15:33Z -->

## Intake summary

既存 Ticket の body/thread/artifacts と関連 closed Tickets、現在の `.yoi/ticket.config.toml` / `.yoi/profiles*` / `resources/profiles/default.lua` を確認した。新規 duplicate は作成しない。Ticket は Project role profiles を builtin profiles へ移行する concrete work item として実装可能で、blocking open questions はない。risk flags / reviewer focus は `profile-manifest`, `runtime-resource`, `authority-boundary`, `tool-surface`, `role-launch-config`, `prompt-boundary`, `packaging-resource`。実装時は builtin selector 解決、role launcher/config、現行 feature/tool policy 維持、project-local profile 残存理由、`nix build .#yoi` まで確認する。

---

<!-- event: state_changed author: intake at: 2026-06-10T10:15:33Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により要件・受け入れ条件・invariants・implementation latitude・validation focus が揃ったため、Orchestrator が routing できる状態として ready にする。

---

<!-- event: decision author: intake at: 2026-06-10T11:01:57Z -->

## Decision

## Decision: builtin role profile と project-specific parameter の分離

`00001KTRG8N9J` の実装方針として、builtin role profiles へ移す対象は **role-level feature/tool policy と reusable Profile defaults** に限定する。現行 project-local profiles をそのまま完全コピーして builtin 化するのではなく、Yoi の role contract / safety boundary と、この workspace 固有の dogfooding parameter を分離する。

Builtin 化してよいもの:
- role ごとの tool surface / feature policy。
  - Intake: Ticket 基本操作あり、Pods / ticket orchestration / Task なし。
  - Orchestrator: Ticket lifecycle / ticket orchestration / Pods を持てる。
  - Coder: 実装用 profile として、実際の権限は launch 側の委譲 scope に従う。
  - Reviewer: review 用で、orchestration / broad lifecycle 操作は持たない。
  - Companion: 相談・状況把握用で、実装・orchestration 権限を持たない。
- role が持つべき抽象 scope の姿勢。ただし concrete path、Pod 名、session state、resolved Manifest、runtime-bound field は含めない。
- `builtin:default` から継承できる Yoi の標準運用値。
- role の `slug` / description / feature flags。
- global `yoi` style の Profile authoring。LLM-facing prompt prose や workflow 本体は Profile に入れない。

Project/user config 側に残すべきもの:
- `ticket.language` や worker language など、この repository の運用言語設定。
- exact model/provider pin。例: `codex-oauth/gpt-5.5` は dogfooding 運用値であり、role contract そのものではない。
- web provider / secret ref。例: Brave provider や `api_key_secret` は環境依存。
- Ticket backend root、Ticket language、workflow mapping、project-specific workflow override。
- 公開前 dogfooding のための一時的な高 reasoning / 特定 model などの project-local override。

実装上の期待:
- builtin role profiles は「この repository 専用の完成済み Profile」ではなく、`builtin:default` を base にした role policy preset として扱う。
- `.yoi/ticket.config.toml` の role selector は `builtin:intake` などへ移行してよいが、project 固有の language/backend/workflow 設定は config 側に残す。
- project-local profiles を削除するか薄い override として残すかは実装時に判断してよい。ただし残す場合は、builtin role contract ではなく project override である理由を実装報告に残す。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-10T11:16:43Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-10T15:00:32Z from: queued to: inprogress reason: orchestrator_queue_acceptance field: state -->

## State changed

Queue acceptance: dirty Ticket records were committed (`a0b688e2`), relation query has no blockers, orchestration-plan query has only the prior temporary waiting note, workspace state is clean, and user explicitly authorized continuing. Implementation side effects may proceed after this recorded acceptance.

---

<!-- event: plan author: orchestrator at: 2026-06-10T15:00:32Z -->

## Plan

Routing decision: implementation_ready

Reason:
- Ticket is `queued`, user authorized continuing, and dirty workspace blocker was resolved by committing unrelated Ticket records in `a0b688e2`.
- Relation query has no blockers; orchestration plan only had the temporary waiting note from the dirty-state gate.
- Ticket body/thread contain concrete requirements, binding decisions, non-goals, acceptance criteria, and reviewer focus.

Evidence checked:
- Ticket body/thread/artifacts.
- `TicketRelationQuery`: no blockers.
- `TicketOrchestrationPlanQuery`: prior waiting note only.
- Workspace state after commit: clean.
- Relevant context: closed `00001KTR6D3C5` already added global `yoi` and `yoi.profile.import/extend`; closed prompt-resource Ticket keeps LLM-facing prompt prose out of Profiles.

IntentPacket:
- Intent: migrate standard project role Profiles into builtin reusable role policy presets and update this workspace Ticket role config to select those builtin Profiles.
- Binding decisions / invariants: builtin Profiles carry role-level feature/tool policy and reusable defaults only; do not embed LLM-facing prompt/workflow prose; do not include runtime-bound fields, concrete paths, Pod/session state, resolved Manifest, concrete delegated scope/path, project language/backend/workflow/model/secret pins, or environment-specific web secret refs as role contract; final artifacts must pass Profile validation.
- Requirements / acceptance criteria: `builtin:companion`, `builtin:intake`, `builtin:orchestrator`, `builtin:coder`, `builtin:reviewer` resolve through the normal builtin Profile registry/resolver; `.yoi/ticket.config.toml` role selectors use builtin selectors; current project-local role profile handling is removed or explicitly justified; role feature/tool policy remains equivalent to current project role policy; tests cover resolver/registry and role config/launcher as needed.
- Implementation latitude: exact Lua file organization and whether project-local profiles are deleted or kept as thin overrides may be chosen during implementation, but the reason must be recorded. Project-specific values may remain in config or local overrides.
- Escalate if implementation requires broad Profile selector/registry redesign, embedding prompt/workflow prose into Profiles, weakening validation, or expanding role tool authority beyond the recorded policy.
- Validation: `cargo fmt`, manifest profile tests, client Ticket role tests if touched, `target/debug/yoi ticket doctor`, and `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-10T15:15:21Z -->

## Implementation report

Implementation report

Branch/worktree: `builtin-role-profiles` / `.worktree/builtin-role-profiles`
Commit:
- `52afdda4 feat: add builtin role profiles`

Summary:
- Added bundled role Profiles under `resources/profiles`: `companion.lua`, `intake.lua`, `orchestrator.lua`, `coder.lua`, `reviewer.lua`.
- Registered those Profiles as builtin selectors: `builtin:companion`, `builtin:intake`, `builtin:orchestrator`, `builtin:coder`, `builtin:reviewer`.
- Role Profiles use global `yoi` / `yoi.profile.extend("builtin:default", ...)` style and keep prompt/workflow prose out of Profile artifacts.
- Migrated this workspace `.yoi/ticket.config.toml` role selectors from `project:*` to `builtin:*` selectors.
- Replaced project-local role Profile files with builtin resource Profiles; `.yoi/profiles.toml` now only points the workspace default at `builtin:companion`.
- Updated Ticket config scaffold defaults to role-specific builtin selectors and adjusted client role launch tests.
- Added manifest tests for builtin role registry/resolution and role tool policy.

Validation run in branch:
- `cargo fmt --check` passed.
- `cargo test -p manifest profile --lib` passed (23 passed).
- `cargo test -p ticket config --lib` passed (20 passed).
- `cargo test -p client ticket_role --lib` passed.
- `cargo check -p manifest -p ticket` passed.
- `git diff --check` passed.
- `nix build .#yoi` passed.

Notes:
- Builtin role Profiles intentionally inherit common reusable defaults from `builtin:default`; project-specific Ticket backend/language/workflow config remains in `.yoi/ticket.config.toml` rather than moving into Profiles.
- The removed project-local role Profile files were policy presets rather than project data, so moving them to embedded resources satisfies the migration goal.

---

<!-- event: review author: reviewer-builtin-role-profiles at: 2026-06-10T15:23:44Z status: request_changes -->

## Review: request changes

Branch-local review verdict: request_changes

Blockers:
- Role Profiles extended `builtin:default`, which leaked `scope = workspace_write()` into companion/intake/orchestrator/reviewer. Reviewer and companion must remain read-only; only coder should have workspace write by role policy.
- Role Profiles inherited `builtin:default` model and web secret values (`codex-oauth/gpt-5.5`, `web/brave/default`), which made project/environment-specific pins part of the reusable role contract.

Validation run by reviewer passed:
- `cargo test -p manifest profile --lib`
- `cargo test -p ticket config --lib`
- `cargo test -p client ticket_role --lib`
- `cargo check -p manifest -p ticket`
- `git diff --check $(git merge-base HEAD develop)..HEAD`
- `nix build .#yoi`

No files were modified by reviewer.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-10T15:23:55Z -->

## Implementation report

Fix-loop implementation report

Reviewer blockers fixed in amended implementation commit `85c06dc6`:
- Role builtin Profiles no longer extend `builtin:default`; they are standalone `yoi.profile { ... }` artifacts.
- `companion`, `intake`, `orchestrator`, and `reviewer` explicitly use `yoi.scope.workspace_read()`.
- `coder` is the only role Profile using `yoi.scope.workspace_write()`.
- Role Profiles no longer inherit `model.ref` or `web.search.api_key_secret` from `builtin:default`.
- Manifest role-policy test now asserts role scope, absence of model ref, and absence of web config for each builtin role Profile.

Validation after fixes:
- `cargo fmt --check` passed.
- `cargo test -p manifest profile --lib` passed (23 passed).
- `cargo test -p ticket config --lib` passed (20 passed).
- `cargo test -p client ticket_role --lib` passed.
- `cargo check -p manifest -p ticket` passed.
- `git diff --check` passed.
- `nix build .#yoi` passed.

Reviewer `reviewer-builtin-role-profiles` has been asked to re-review the amended branch.

---

<!-- event: review author: reviewer-builtin-role-profiles at: 2026-06-10T15:26:06Z status: approve -->

## Review: approve

Branch-local review verdict: approve

Blockers: none.

Reviewer confirmed previous blockers are resolved:
- Role builtin Profiles are standalone `yoi.profile { ... }` artifacts rather than extending `builtin:default`.
- Prompt/workflow prose, model ref, and web secret ref are not embedded in role builtin Profiles.
- `companion` / `intake` / `orchestrator` / `reviewer` use read scope; `coder` is the only write-scope role; `orchestrator` keeps reusable `delegation_scope = workspace_write()` intent.

Non-blocking notes:
- Some custom concrete profile tests still use `builtin:default`, but scaffold/role-default launch tests now verify role-specific builtin defaults.

Validation run by reviewer:
- `cargo fmt --check` passed.
- `cargo test -p manifest profile --lib` passed.
- `cargo test -p ticket config --lib` passed.
- `cargo test -p client ticket_role --lib` passed.
- `cargo check -p manifest -p ticket` passed.
- `git diff --check $(git merge-base HEAD develop)..HEAD` passed.
- `nix build .#yoi` passed.

This is branch-local review evidence; no final main-branch approval/close was recorded by reviewer.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-10T15:26:32Z -->

## Implementation report

Merge-ready dossier

Ticket id: `00001KTRG8N9J`
Branch/worktree: `builtin-role-profiles` / `.worktree/builtin-role-profiles`
Commits:
- `85c06dc6 feat: add builtin role profiles`

Intent / invariant check:
- Standard role policy presets are moved from project-local `.yoi/profiles` into embedded builtin Profiles under `resources/profiles`.
- Builtin role Profiles contain reusable role feature/tool/scope policy only.
- Prompt/workflow prose, runtime-bound fields, concrete paths, Pod/session state, project Ticket backend/language/workflow config, model pins, and web secret refs are not embedded in role builtin Profiles.

Implementation summary:
- Added `builtin:companion`, `builtin:intake`, `builtin:orchestrator`, `builtin:coder`, and `builtin:reviewer` to the builtin Profile registry/resolver path.
- Added standalone global-`yoi` Lua Profile resources for each role.
- Migrated `.yoi/ticket.config.toml` role selectors to builtin selectors.
- Removed project-local role Profile Lua files; `.yoi/profiles.toml` now only points default to `builtin:companion`.
- Updated Ticket config scaffold and client role launch expectations to role-specific builtin defaults.
- Added tests for builtin role registration/resolution and role policy boundaries.

Coder/reviewer Pods:
- Implementation was done directly by Orchestrator in the worktree because this session previously could not delegate write scope to coder Pods.
- Reviewer Pod: `reviewer-builtin-role-profiles`.

Blockers fixed or rejected findings:
- Fixed reviewer blocker: role Profiles no longer inherit `builtin:default` write scope, model ref, or web secret.
- Fixed reviewer blocker: read/write role scope and absence of model/web config are now asserted in tests.
- No remaining blockers. Non-blocking: some custom concrete profile tests still use `builtin:default`, which is acceptable because role-default scaffold/launch tests cover the migrated default role selectors.

Validation performed:
- Orchestrator after fixes: `cargo fmt --check`, `cargo test -p manifest profile --lib`, `cargo test -p ticket config --lib`, `cargo test -p client ticket_role --lib`, `cargo check -p manifest -p ticket`, `git diff --check`, `nix build .#yoi` passed.
- Reviewer after fixes: same focused validation plus `git diff --check $(git merge-base HEAD develop)..HEAD` and `nix build .#yoi` passed.

Residual risks:
- Low. Main remaining risk is whether broad future user/project profile import is desired; this Ticket only requires builtin role selector migration and current role policy boundaries.

Dirty state:
- Main workspace has only Ticket review/dossier records pending commit at dossier time.
- Worktree branch is clean.

Parent/human decision needs:
- Panel launch guidance for this Ticket said to stop at merge-ready dossier without merge/close/final approval. Explicit merge-completion authorization is needed before merging `builtin-role-profiles`, running post-merge validation, closing the Ticket, and cleaning the worktree/branch.

---

<!-- event: review author: orchestrator at: 2026-06-11T00:05:00Z status: approve -->

## Review: approve

Main-branch review/merge-completion approval.

Verified before merge:
- Merge-ready dossier matched Ticket `00001KTRG8N9J`, branch `builtin-role-profiles`, worktree `.worktree/builtin-role-profiles`, and implementation commit `85c06dc6`.
- Branch-local reviewer `reviewer-builtin-role-profiles` approved after blocker fixes.
- Main workspace was clean before merge.

Merged:
- `git merge --no-ff builtin-role-profiles -m "merge: builtin role profiles"`
- Merge commit: `7daecca8 merge: builtin role profiles`

Post-merge validation:
- `cargo fmt --check` passed.
- `git diff --check` passed.
- `cargo test -p manifest profile --lib` passed.
- `cargo test -p ticket config --lib` passed.
- `cargo test -p client ticket_role --lib` passed.
- `cargo check -p manifest -p ticket` passed.
- `target/debug/yoi ticket doctor` passed.
- typed `TicketDoctor` reported 0 errors and 3 pre-existing diagnostics.
- `nix build .#yoi` passed.

Result: approve.

---

<!-- event: state_changed author: orchestrator at: 2026-06-11T00:05:00Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Merged branch `builtin-role-profiles` with merge commit `7daecca8`; branch-local review approved after fixes and post-merge validation passed (`cargo fmt --check`, `git diff --check`, `cargo test -p manifest profile --lib`, `cargo test -p ticket config --lib`, `cargo test -p client ticket_role --lib`, `cargo check -p manifest -p ticket`, `target/debug/yoi ticket doctor`, `nix build .#yoi`).

---

<!-- event: state_changed author: hare at: 2026-06-11T00:05:19Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-11T00:05:19Z status: closed -->

## 完了

完了しました。

実施内容:
- `builtin:companion` / `builtin:intake` / `builtin:orchestrator` / `builtin:coder` / `builtin:reviewer` を builtin Profile として追加しました。
- role Profile は `resources/profiles/*.lua` に移動し、global `yoi` style の standalone `yoi.profile { ... }` artifact として登録しました。
- `builtin:default` 由来の model ref / web secret / workspace write scope が role contract に混入しないよう、role Profiles は `builtin:default` を extend しない形にしました。
- `companion` / `intake` / `orchestrator` / `reviewer` は read scope、`coder` のみ write scope、`orchestrator` は reusable delegation intent として `delegation_scope = workspace_write()` を持ちます。
- `.yoi/ticket.config.toml` の role selectors を `project:*` から `builtin:*` に移行しました。
- project-local role Profile files を削除し、`.yoi/profiles.toml` は workspace default を `builtin:companion` に向けるだけに整理しました。
- Ticket config scaffold と client role launch tests を role-specific builtin defaults に合わせて更新しました。
- manifest tests に builtin role registry/resolution と role policy boundary の検証を追加しました。

Merge:
- Branch: `builtin-role-profiles`
- Implementation commit: `85c06dc6 feat: add builtin role profiles`
- Merge commit: `7daecca8 merge: builtin role profiles`

確認:
- Branch-local reviewer `reviewer-builtin-role-profiles` が初回 request_changes 後、修正済み branch を approve。
- `cargo fmt --check` passed。
- `git diff --check` passed。
- `cargo test -p manifest profile --lib` passed。
- `cargo test -p ticket config --lib` passed。
- `cargo test -p client ticket_role --lib` passed。
- `cargo check -p manifest -p ticket` passed。
- `target/debug/yoi ticket doctor` passed。
- typed `TicketDoctor` は 0 errors / 3 pre-existing diagnostics。
- `nix build .#yoi` passed。

残作業:
- なし。将来、role builtin Profile とは別に broad user/project profile import selector が必要になった場合は follow-up Ticket として扱えます。

---
