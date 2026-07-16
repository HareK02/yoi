<!-- event: create author: "yoi ticket" at: 2026-07-15T19:43:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: comment author: hare at: 2026-07-15T19:56:41Z -->

## Comment

## Current implementation check: Skills support

現行の Skills support は存在するが、first-class Skill ではなく **Workflow ingestion の一部**として実装されている。

### 既にあるもの

- `crates/workflow/src/skill.rs` に `SKILL.md` parser がある。
  - `<skills-root>/<name>/SKILL.md` を読む。
  - YAML frontmatter + Markdown body を要求する。
  - `name` / `description` は required。
  - `name` は parent directory name と一致する必要がある。
  - `name` は既存 `Slug` validation を通る必要がある。
  - `description` は non-empty かつ `WORKFLOW_DESCRIPTION_HARD_CAP` 以内。
  - `license` / `compatibility` / `metadata` / `allowed-tools` は parse 上は受理する。
  - unknown frontmatter fields は warning で無視する。
  - `allowed-tools` は warning を出すだけで enforcement しない。
- `load_skills_from_dir(root)` がある。
  - root 直下の directory だけを見て、各 `<name>/SKILL.md` を読む。
  - broken skill は warning で skip し、siblings は読み続ける。
  - missing root は empty 扱い。
  - deterministic order で読み込む。
- `manifest` に `[skills] directories = [...]` がある。
  - path は manifest/profile base から resolve される。
  - validation 後は absolute path required。
  - profile/manifest merge で directories は extend される。
- Worker startup で `[skills].directories` が scope read allow に追加される。
  - skill root は recursive read allow になる。
  - `SKILL.md` / `scripts/` / `references/` / `assets/` 全体を通常 Read できる。
- Worker startup で Skills が workflow registry に取り込まれる。
  - `SkillRecord::into_workflow_record()` により Skill が `WorkflowRecord` に変換される。
  - `model_invokation = true`、`user_invocable = true`。
  - つまり Skill は `/<slug>` で呼べる Workflow として扱われる。
- shadowing support がある。
  - internal/builtin/workspace Workflow が同 slug を持つ Skill より優先される。
  - 複数 skill directories 間では first-fed wins。
  - shadowed Skill は Worker notification に `[Skill shadowed] ...` として流れる。
- tests は parser / directory loading / Workflow projection / scope allow / ingest_skills をある程度カバーしている。

### 現行の限界

- Skill は first-class ではなく、WorkflowRecord へ projection されている。
  - Skill catalog / Skill activation / Skill provenance は Workflow registry に埋もれる。
  - Workflow 削除 Ticket と衝突する構造になっている。
- `.yoi/skills/` は implicit workspace Skill root ではない。
  - 現在は `[skills].directories` に明示された root だけを読む。
  - default workspace path として `.yoi/skills` を probe しない。
- builtin skills はない。
  - `resources/skills/<name>/SKILL.md` のような bundled layout は未定義。
- Progressive disclosure は不完全。
  - metadata は resident workflow entry として出るが、Skill catalog と full body activation が分離されていない。
  - full `SKILL.md` body は workflow invocation body として扱われるだけ。
  - `references/` / `scripts/` / `assets/` はただ read-scope に入るだけで、Skill-aware access/tooling はない。
- `allowed-tools` は authority に反映されない。
  - parser は認識するが warning のみ。
  - feature/tool permission と合成しない。
- frontmatter optional fields は保持されない。
  - `license` / `compatibility` / `metadata` は parse acceptance のみで、SkillRecord / catalog には残らない。
- lint surface が独立していない。
  - parser tests はあるが、`yoi skill lint` のような user-facing lint/check はない。
- Skill resource safety は最小限。
  - skill root を recursive read allow にしているだけ。
  - relative references / nested reference chain / script execution policy は未実装。
- Web Workspace UI / backend API はない。
  - `.yoi/skills` の list / validation / edit surface は未実装。
- current docs/comments は Skill を Workflow ingestion として説明しており、今後の方針と逆。
  - `crates/workflow/src/skill.rs` の module docs も「ingests it as a Workflow」と明記している。

### 現在地まとめ

現在の Skills support は **Agent Skills parser + manifest-configured directory ingest + Workflow compatibility projection** まで。Agent Skills 標準の directory shape と basic frontmatter validation は部分的にあるが、Yoi の Skill としての lifecycle / catalog / activation / provenance / lint / UI / authority boundary は未実装。

この Ticket の実装では、まず Workflow projection から切り離して、`.yoi/skills/<skill-name>/SKILL.md` を workspace Skill authority として扱う first-class Skill loader/catalog へ移す必要がある。


---

<!-- event: decision author: hare at: 2026-07-15T20:43:42Z -->

## Decision

Workspace backend を Skill discovery / lint / catalog / activation の authority とする。

`.yoi/skills/` は tracked workspace data の保存場所だが、Runtime Worker が各自で直接 scan して別々の Skill view を持つ構造にはしない。Web Workspace UI、Runtime Worker、embedded Worker、CLI は Workspace backend API から同じ catalog / diagnostics / activation body を取得する。

これにより、Skill の override/provenance/lint diagnostics、Web からの編集・検証、Worker への progressive disclosure を同じ authority に寄せる。


---

<!-- event: decision author: hare at: 2026-07-15T21:02:33Z -->

## Decision

Skill support should mirror the Ticket backend direction: Workspace backend owns the shared authority and exposes typed APIs for Worker / Runtime process / Web / CLI access.

Required API surface includes catalog/list, detail/read, lint/diagnostics, activation body retrieval, and resource access or backend-resolved authority for references/assets. Worker-local filesystem scanning must not become the primary authority when `WorkspaceClient::Http` is available.


---

<!-- event: intake_summary author: hare at: 2026-07-15T21:17:01Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-15T21:17:01Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-15T21:26:15Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-15T21:26:58Z -->

## Decision

Routing decision: blocked_by_dependency

Reason:
- Dashboard queue authorization was inspected, but this Ticket has explicit `depends_on` blockers。
- `00001KXKJGYGD` (`Remove workflow tracking and workflow resources`) is currently `inprogress` and still in reviewer/coder fix loop。
- `00001KXKP2A71` (`Remove Knowledge support`) is still `queued` and was intentionally held until Workflow removal completes。
- This Ticket’s body depends on both: Workflow projection must be gone, and obsolete Knowledge support should be removed before first-class Agent Skills support introduces new Skill/Knowledge boundaries and Workspace Skill APIs。
- Therefore this routing pass leaves the Ticket `queued` and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / thread / relations。
- `TicketRelationQuery(00001KXKMX0QM)`: `depends_on 00001KXKJGYGD` and `depends_on 00001KXKP2A71`。
- Ticket derived blocker view: `00001KXKJGYGD` is `inprogress`, `00001KXKP2A71` is `queued`。
- `TicketOrchestrationPlanQuery(00001KXKMX0QM)`: no prior records; this pass recorded `after` entries and waiting note。
- `TicketList(inprogress)`: `00001KXKJGYGD` active。
- Orchestrator worktree status: clean。

Next action:
- Finish `00001KXKJGYGD` first。
- Re-route and complete `00001KXKP2A71` second。
- Re-route this Skills Ticket after both blockers are approved, merged, validated, and closed。

Escalate if:
- Human explicitly requests a combined branch across Workflow removal / Knowledge removal / Skills support and accepts the larger review-boundary risk。

---

<!-- event: decision author: orchestrator at: 2026-07-15T23:01:21Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Explicit dependencies are now satisfied: `00001KXKJGYGD` Workflow removal and `00001KXKP2A71` Knowledge removal are both approved, merged, final-validated, closed, and their implementation worktrees/branches cleaned。
- `TicketRelationQuery(00001KXKMX0QM)` still records the outgoing `depends_on` edges, but the derived blocker view is empty because both targets are closed。
- `TicketOrchestrationPlanQuery(00001KXKMX0QM)` contained earlier waiting notes for those blockers; this pass records an accepted implementation plan。
- `TicketList(inprogress)` is 0 件。
- Orchestrator worktree is clean and no existing `00001KXKMX0QM` implementation worktree/branch was found。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- Closed prerequisite Tickets `00001KXKJGYGD` and `00001KXKP2A71`。
- Orchestrator worktree state / worktree list / branch list。

IntentPacket:

Intent:
- Implement first-class Agent Skills support using Workspace backend authority。
- `.yoi/skills/<skill-name>/SKILL.md` becomes tracked workspace Skill data, but Worker-local filesystem scan must not be the primary authority when Workspace backend/client is available。
- Provide shared Skill catalog/lint/read/activation APIs for Worker / Runtime / Web / CLI and progressive disclosure of Skill metadata vs body/resources。

Binding decisions / invariants:
- Skill is LLM-facing procedural guidance/resource, not external state authority, scheduler, script runner, queue runner, or worktree manager。
- Ticket / Worker / workdir / repository / network external state changes remain in typed feature/tool surfaces。
- Workflow projection semantics must not return: no `WorkflowRecord`, `model_invokation`, `user_invocable`, graph/invocation assumptions, or `/workflow-slug` compatibility path。
- Knowledge has been removed as active support; do not rebuild Knowledge under Skill terminology。
- Workspace backend is authority for Skill discovery/lint/catalog/activation。
- Worker with `WorkspaceClient::Http` must receive Skill metadata/body via Workspace API, not local path scan。
- Full `SKILL.md` body is loaded into Worker history/context only on activation/select, not always with the catalog。
- `allowed-tools` is experimental; if implemented, it must not become independent authority without explicit integration with feature/tool permissions。

Requirements / acceptance criteria:
- `.yoi/skills/<skill-name>/SKILL.md` is parsed/linted per Agent Skills required frontmatter and naming rules。
- Workspace backend exposes Skill catalog/list, detail/read, lint/diagnostics, activation body, and resource access or backend-resolved resource authority as appropriate。
- Builtin/workspace loading, override priority, provenance, invalid diagnostics, and non-default/workspace skill cases are covered by tests。
- Worker / Web / CLI use the same Workspace backend Skill catalog view。
- Activation appends/commits Skill body into Worker history before LLM context use。
- Progressive disclosure is tested: catalog metadata is lightweight, full body/resources load only when requested/activated。
- Role prompts/internal prompts/docs are updated to Skill terminology and no Workflow/Knowledge active guidance remains。
- Initial implementation clearly marks unsupported decision points such as `allowed-tools`/scripts/resource execution if not implemented。

Implementation latitude:
- Exact crate/module placement and API DTO shapes may follow current workspace-server / Worker WorkspaceClient patterns。
- Builtin Skill resource layout can be chosen based on existing resource conventions, but must be tested and documented。
- Workspace override policy may choose complete override or conflict diagnostic if documented and tested; prefer a simple deterministic rule。
- Web UI editing can be deferred if backend/API/CLI tests cover catalog/lint/read/activation and Ticket does not require full editor implementation。

Escalate if:
- Skill activation syntax (`/skill-name` vs explicit tool/API/UI activation) requires product decision beyond existing Ticket text。
- Supporting `scripts/` execution would require new authority/sandbox model; prefer unsupported/diagnostic unless explicitly scoped。
- Implementing resource access needs broad backend protocol design beyond bounded read/detail endpoints。
- Any path requires reintroducing Workflow or Knowledge active surfaces。

Validation:
- `git diff --check`
- Skill loader/lint/catalog tests in affected crate(s)。
- `cargo test -p yoi-workspace-server --lib` if Workspace API is touched。
- `cargo test -p worker --lib --tests` if Worker prompt/history/WorkspaceClient activation is touched。
- `cargo test -p yoi --tests` if CLI/input activation is touched。
- `cd web/workspace && deno task check && deno task test` if web/API types are touched。
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Worker-local scan becoming divergent authority。
- Full Skill body being resident by default rather than activated/progressive。
- Workflow/Knowledge surfaces returning under new names。
- `allowed-tools` or scripts accidentally becoming authority bypass。
- Skill resources leaking raw paths or bypassing Workspace backend authority。
- External state control moving into Skill text instead of typed tools/features。

---

<!-- event: state_changed author: orchestrator at: 2026-07-15T23:01:28Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Dependencies Workflow removal and Knowledge removal are closed, no blockers remain, accepted plan recorded. Moving queued Ticket to inprogress before creating worktree or spawning role Pods.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-15T23:46:45Z -->

## Implementation report

Implementation progress report:

- Coder Pod `yoi-coder-00001KXKMX0QM-agent-skills` completed implementation in `/home/hare/Projects/yoi/.worktree/00001KXKMX0QM-agent-skills-support` on branch `work/00001KXKMX0QM-agent-skills-support`。
- Implementation commit: `62ef89a1634fd12a503156ce95baebbf037ef96f` (`feat: add workspace-backed agent skills`)。
- Orchestrator inspected worktree status, branch log, commit stats, and `git diff --check 05c50e32..HEAD`; worktree was clean and diff check passed。

Implementation summary:
- Added first-class Skill schema/types and Workspace HTTP client support in `worker`。
- Added Workspace backend Skill discovery/lint/catalog/detail/activation in `yoi-workspace-server`。
- Added builtin Skill resource at `resources/skills/agent-skills/SKILL.md`。
- Added Workspace server API endpoints:
  - `GET /api/w/{workspace_id}/skills`
  - `GET /api/w/{workspace_id}/skills/lint`
  - `GET /api/w/{workspace_id}/skills/{name}`
  - `GET /api/w/{workspace_id}/skills/{name}/activate`
- Added `yoi-workspace-server skills list|lint|show` CLI surface。
- Added Web Workspace API helpers/types for Skill catalog/detail/activation endpoints。
- Added Worker Skill activation method using `WorkspaceClient::Http`; activation commits a `SystemItem::SkillActivation` and appends the Skill body into engine history before later LLM context use。
- Added prompt guidance clarifying Skills are procedural LLM guidance only, not authority for external state changes。

Skill behavior:
- Workspace Skills load from `.yoi/skills/<skill-name>/SKILL.md`。
- Lint enforces required `name` / `description`, parent-dir match, name length/pattern, description bounds, optional `license` / `compatibility` / string-map `metadata`。
- `allowed-tools` is parsed as metadata but emits explicit ignored/non-authoritative warning。
- Builtin + workspace Skills are loaded by Workspace backend; workspace Skills deterministically override builtin Skills with path-free provenance such as `builtin:agent-skills` / `workspace:debug-rust`。
- Catalog responses contain lightweight metadata only; full `SKILL.md` body is returned only from detail/activation endpoints。
- References/assets/scripts are Skill-relative resource refs only; no raw absolute paths are exposed. Resource reads/execution are not implemented, scripts are explicitly non-executable diagnostics。
- No `/skill-name` activation syntax, Workflow projection/compatibility path, or Knowledge active support was added。

Files/resources touched:
- `crates/workspace-server/src/skills.rs`
- `crates/workspace-server/src/server.rs`
- `crates/workspace-server/src/main.rs`
- `crates/workspace-server/src/lib.rs`
- `crates/worker/src/skill.rs`
- `crates/worker/src/worker.rs`
- `crates/worker/src/lib.rs`
- `crates/session-store/src/system_item.rs`
- `crates/tui/src/app.rs`
- `web/workspace/src/lib/workspace/api/http.ts`
- `web/workspace/src/lib/workspace/api/http.test.ts`
- `resources/skills/agent-skills/SKILL.md`
- `resources/prompts/common/tool-usage.md`

Coder-reported validation passed:
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker --lib --tests`
- `cargo check -p yoi-workspace-server`
- `cargo check -p yoi`
- `cargo test -p yoi --tests`
- `cd web/workspace && deno task check && deno task test` (`54 passed`, existing App.svelte warnings only)
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`
- `rg "WorkflowRecord|model_invokation|user_invocable|KnowledgeQuery|kind = knowledge|#<slug>|workflow_invoke"`: no active regressions found。

Next action:
- Route to external Reviewer Pod before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-15T23:53:00Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blockers:

1. Workflow/unsupported Skill frontmatter is silently accepted instead of linted/diagnosed。
   - A `.yoi/skills/*/SKILL.md` containing `model_invokation`, `user_invocable`, graph/invocation fields, etc. will deserialize successfully and can be returned/activated as body。
   - `SkillFrontmatter` only lists known fields and uses default serde unknown-field ignoring at `crates/workspace-server/src/skills.rs:40-50`, with parsing around `:290-300`。
   - This violates the acceptance that workflow projection fields are not accepted as active Skill semantics。

2. Required Worker activation/history behavior lacks coverage。
   - `Worker::activate_skill` commits/appends at `crates/worker/src/worker.rs:906-922`。
   - Worker Skill tests only exercise HTTP catalog fetch at `crates/worker/src/skill.rs:194-237`。
   - Missing the stated acceptance test that activation appends/commits Skill body into Worker history before LLM context use。

Evidence / validation performed:
- Inspected Ticket and touched files。
- Checked route/API/provenance behavior。
- Ran `cargo test -p yoi-workspace-server skills --lib`。
- Ran `cargo test -p worker skill --lib`。
- Confirmed implementation worktree remained clean。

Non-blocking note:
- Catalog/detail progressive disclosure and path-free provenance generally look aligned。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-16T00:07:38Z -->

## Implementation report

Follow-up implementation progress report after request_changes:

- Coder Pod `yoi-coder-00001KXKMX0QM-agent-skills` completed the review fixes in `/home/hare/Projects/yoi/.worktree/00001KXKMX0QM-agent-skills-support`。
- Follow-up commit: `4ddfccee2d414a2dfe67a940906b505dd237132d` (`fix: reject unsupported skill fields`)。
- Orchestrator inspected worktree status, branch log, commit stats, and `git diff --check 05c50e32..HEAD`; worktree was clean and diff check passed。

Fix summary:
- Added Skill frontmatter unsupported-key detection in `crates/workspace-server/src/skills.rs`。
- Unknown frontmatter keys are now lint errors。
- Workflow/projection/invocation-shaped keys such as `model_invokation`, `user_invocable`, `graph`, and `invocation` are explicitly rejected with `unsupported_workflow_frontmatter_field`。
- Unsupported/unknown-field Skills no longer become active catalog/detail/activation entries。
- Added regression tests for `model_invokation`, `user_invocable`, `graph`, `invocation`, and generic unknown frontmatter fields。
- Added Worker activation/history coverage in `crates/worker/src/worker.rs`:
  - verifies `Worker::activate_skill` fetches activation through Workspace HTTP client;
  - verifies Skill body is appended to Worker engine history;
  - verifies committed `SystemItem::SkillActivation` content matches the history-visible Skill activation content。

Coder-reported validation passed:
- `cargo test -p yoi-workspace-server skills --lib`
- `cargo test -p worker skill --lib`
- `git diff --check`
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Web/API types were not changed in this follow-up, so web checks were not rerun。

---

<!-- event: review author: reviewer at: 2026-07-16T00:10:51Z status: approve -->

## Review: approve

Focused re-review result: approve

Evidence:
- Previous blockers fixed:
  - Skill frontmatter is now parsed through a raw YAML value。
  - Unsupported/workflow-shaped keys are diagnosed as errors (`crates/workspace-server/src/skills.rs:290-302`, `457-528`)。
  - Error diagnostics prevent catalog/detail/activation (`skills.rs:393-398`)。
  - Regression coverage exists for `model_invokation`, `user_invocable`, `graph`, `invocation`, and generic unknown fields (`skills.rs:687-743`)。
  - Worker activation/history is covered by an HTTP-backed activation test verifying backend fetch, history append, and committed `SystemItem::SkillActivation` body equality (`crates/worker/src/worker.rs:5971-6065`)。
- Focused validation passed:
  - `cargo test -p yoi-workspace-server skills --lib`
  - `cargo test -p worker skill --lib`
  - `git diff --check`
- Worktree remained clean。
- Rechecked acceptance focus:
  - Workspace backend remains the Skill catalog/detail/lint/activation authority。
  - builtin/workspace override/provenance/progressive disclosure behavior remains covered。
  - `allowed-tools` / scripts remain diagnostic/non-authoritative。
  - no raw-path resource leakage was introduced。
  - `rg` for removed Workflow/Knowledge surfaces only finds new rejection logic/tests。

Blockers: none。

Non-blocking note:
- detail/activation for invalid Skills still surface as not-found while catalog/lint carries diagnostics; acceptable for this initial authority model。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-16T00:14:34Z -->

## Implementation report

Merge / final validation report:

- Reviewer approved the implementation after unsupported Skill frontmatter and Worker activation/history fixes。
- Merged branch `work/00001KXKMX0QM-agent-skills-support` into orchestration branch with merge commit `1611e04d merge: agent skills support`。
- Final validation was rerun in `/home/hare/Projects/yoi/.worktree/orchestration` after merge。

Validation passed:
- Regression grep for `WorkflowRecord|model_invokation|user_invocable|KnowledgeQuery|kind = knowledge|#<slug>|workflow_invoke` with only expected Skill rejection logic/tests where applicable。
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker --lib --tests`
- `cargo test -p yoi --tests`
- `cd web/workspace && deno task check`
- `cd web/workspace && deno task test`
- `cargo check -p yoi-workspace-server`
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/agent-skills-final-validation-1784160679.txt`

Reviewed/merged implementation commits:
- `62ef89a1 feat: add workspace-backed agent skills`
- `4ddfccee fix: reject unsupported skill fields`

Notes:
- Workspace backend is the Skill catalog/detail/lint/activation authority。
- Workspace Skills live under `.yoi/skills/<skill-name>/SKILL.md`; builtin Skill resources are supported with deterministic workspace override/provenance diagnostics。
- Catalog is lightweight; detail/activation returns full body。
- Worker activation through `WorkspaceClient::Http` commits/appends Skill body to history before later LLM context use。
- `allowed-tools` and scripts/resources are diagnostic/non-authoritative in this initial implementation。
- Workflow and Knowledge active surfaces were not reintroduced。
- Per user instruction, no `StopPod` cleanup was performed。

---

<!-- event: state_changed author: orchestrator at: 2026-07-16T00:14:40Z from: inprogress to: done reason: implementation_validated field: state -->

## State changed

Implementation was reviewed, merged into orchestration branch, and final validation passed. Moving to done before closure.

---

<!-- event: state_changed author: hare at: 2026-07-16T00:15:05Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-16T00:15:05Z status: closed -->

## 完了

Agent Skills support を実装・レビュー・merge・検証した。

実装内容:
- first-class Skill schema/types と Workspace HTTP client support を追加。
- Workspace backend に Skill discovery / lint / catalog / detail / activation を追加。
- Workspace API endpoints を追加:
  - `GET /api/w/{workspace_id}/skills`
  - `GET /api/w/{workspace_id}/skills/lint`
  - `GET /api/w/{workspace_id}/skills/{name}`
  - `GET /api/w/{workspace_id}/skills/{name}/activate`
- `yoi-workspace-server skills list|lint|show` CLI surface を追加。
- Web Workspace API helpers/types を追加。
- builtin Skill resource `resources/skills/agent-skills/SKILL.md` を追加。
- Worker Skill activation method を追加し、`WorkspaceClient::Http` 経由で Skill activation を取得し、`SystemItem::SkillActivation` を commit し、Skill body を engine history に append するようにした。
- Prompt guidance を更新し、Skills は procedural LLM guidance であり external state authority ではないことを明示。

Skill behavior:
- Workspace Skills are read from `.yoi/skills/<skill-name>/SKILL.md`。
- Lint validates required `name` / `description`, parent-dir match, name length/pattern, description bounds, optional `license` / `compatibility` / string-map `metadata`。
- Unknown frontmatter keys are lint errors。
- Workflow/projection/invocation-shaped keys such as `model_invokation`, `user_invocable`, `graph`, `invocation` are explicitly rejected with `unsupported_workflow_frontmatter_field`。
- `allowed-tools` is parsed/diagnosed as experimental ignored/non-authoritative metadata。
- Builtin and workspace Skills are loaded deterministically; workspace Skills override builtin Skills with path-free provenance。
- Catalog responses contain lightweight metadata only; detail/activation returns full `SKILL.md` body。
- References/assets/scripts are Skill-relative and non-executable/non-authoritative; no raw absolute paths are exposed。
- No `/skill-name` syntax, Workflow compatibility path, or Knowledge active support was added。

Review:
- Initial review requested changes for silently accepted unsupported Workflow frontmatter and missing Worker activation/history coverage。
- `4ddfccee fix: reject unsupported skill fields` added unsupported-key diagnostics/regression tests and Worker activation/history tests。
- Focused re-review approved with no blockers。
- Non-blocking note: detail/activation for invalid Skills returns not-found while catalog/lint carries diagnostics; acceptable for this initial authority model。

Merge / validation:
- Merge commit: `1611e04d merge: agent skills support`。
- Final validation passed:
  - Regression grep for removed Workflow/Knowledge active surfaces。
  - `git diff --check`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p yoi --tests`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `cargo check -p yoi-workspace-server`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/agent-skills-final-validation-1784160679.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。

---
