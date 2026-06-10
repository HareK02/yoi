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
