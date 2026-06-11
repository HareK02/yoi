<!-- event: create author: "yoi ticket" at: 2026-06-10T11:16:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: ticket-intake at: 2026-06-10T11:18:41Z -->

## Plan

## Intake refinement

既存 Ticket を読み、重複作成はしない方針で確認した。この Ticket は loader 実装そのものではなく、現行 project workflow を public builtin 候補と dogfood/project-local 運用へ分離するための設計・監査 work item として扱う。

Readiness classification: `spike_needed` / planning-ready。Orchestrator は実装開始ではなく、まず workflow content audit と source/provenance/priority/resource placement の設計判断を行う planning/spike として route できる。後続で loader 実装や workflow content 移行が必要になった場合は、この Ticket の結論から concrete implementation Ticket に分割する。

Risk flags:

- `prompt-resource`: workflow 文言は LLM-facing prompt resource と同等に扱う必要がある。
- `workflow-source`: builtin / workspace / skill の provenance と priority を誤ると override semantics が曖昧になる。
- `public-api`: public builtin workflow の slug と resident advertisement はユーザー向け contract になり得る。
- `dogfood-boundary`: Yoi repository 固有の Git/worktree/cargo/nix 運用を public default に混ぜない。
- `migration-compat`: `.yoi/ticket.config.toml` の既存 workflow selectors と legacy `ticket-preflight-workflow` 名の扱いを決める必要がある。

Open questions: blocking な追加質問はない。設計中に複数案が残る場合は、workspace override が builtin より優先されること、LLM-facing text を Rust code に直書きしないこと、optional Git/worktree workflow を default resident にしないことを invariant として扱い、未決定点は後続 Ticket 化または decision record 化する。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-10T11:18:55Z -->

## Intake summary

既存 Ticket 00001KTRKZ14C を重複作成せず精査した。対象は現行 `.yoi/workflow/*.md` を public builtin 候補と Yoi repository dogfood/project-local 運用へ分離する設計・監査 work item。Orchestrator は実装開始ではなく planning/spike として route し、workflow content audit、`WorkflowSource::Builtin` 等の provenance、builtin/workspace/skill priority、resident advertisement、resource placement、`.yoi/ticket.config.toml` migration、廃止語彙削除方針を決める。blocking open question はないが、risk flags は prompt-resource / workflow-source / public-api / dogfood-boundary / migration-compat。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-10T11:18:55Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により、Orchestrator が planning/spike routing できる情報が揃った。実装 side effect はまだ開始しない。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-11T02:23:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-11T02:24:22Z from: queued to: inprogress reason: orchestrator_queue_acceptance field: state -->

## State changed

Queue acceptance: relation query has no blockers, orchestration plan has no existing records, and the only dirty files are this Ticket's queued state update from workspace panel. This Ticket is a design/audit work item rather than builtin workflow loader implementation; implementation side effects will be limited to durable design/audit artifacts and Ticket records in a dedicated worktree before branch-local review.

---

<!-- event: plan author: orchestrator at: 2026-06-11T02:24:22Z -->

## Plan

Routing decision: implementation_ready as design/audit artifact work

Reason:
- Ticket is `queued` with no relation blockers and no existing orchestration plan records.
- The requested work is explicitly not builtin workflow loader implementation; it is a design/audit work item whose side effects should be durable project design/audit records and Ticket planning records.
- User/panel authorized routing. The only dirty files before acceptance were this Ticket's workspace-panel queued state updates.

Evidence checked:
- Ticket body/thread/artifacts and intake refinement.
- `TicketRelationQuery`: no blockers.
- `TicketOrchestrationPlanQuery`: no existing records.
- Workspace/worktree state: main only, no active implementation worktrees.
- Visible Pods: only unrelated/restorable Intake peers and self; no active child implementation Pods.

IntentPacket:
- Intent: separate current project-local workflow content into public builtin candidates and Yoi dogfood/project-local operational material, and record the source/provenance/resource/slug/resident/migration decisions needed before implementing builtin workflow loading.
- Binding decisions / invariants: do not implement builtin workflow loader in this Ticket; do not move workflow prose into Rust code; workspace workflows must remain able to override builtin workflows; public default workflows must not embed this repository's Git/worktree/cargo/nix/dogfood procedures; optional Git/worktree isolation must not become default resident unless explicitly enabled; stale `Action required` / `Attention required` terminology must be removed or planned for removal.
- Requirements / acceptance criteria: audit all current `.yoi/workflow/*.md`; classify public-builtin vs dogfood-local material; record `WorkflowSource::Builtin`/provenance design; record source priority including workspace/builtin/skill order; decide resource placement under prompt resources or justify alternative; decide core/optional resident policy; decide slug and `.yoi/ticket.config.toml` migration plan; identify follow-up implementation Tickets; `target/debug/yoi ticket doctor` passes.
- Implementation latitude: exact artifact path/format may be chosen if durable and reviewable; docs/design or Ticket artifact/comment are acceptable, but avoid broad unrelated doc rewrites.
- Escalate if a binding product/API decision cannot be narrowed to a documented proposal, or if implementation appears to require loader/code changes in this Ticket.
- Validation: `target/debug/yoi ticket doctor`, `git diff --check`; docs-only/Ticket-only changes can skip `nix build .#yoi` with explicit reason unless runtime resources/code are touched.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-11T02:27:17Z -->

## Implementation report

Implementation report

Branch/worktree: `workflow-public-dogfood-split` / `.worktree/workflow-public-dogfood-split`
Commit:
- `96a3500b docs: split public and dogfood workflows`

Summary:
- Added `docs/design/workflows-public-dogfood-split.md` as the durable design/audit artifact for this Ticket.
- Audited current `.yoi/workflow/*.md` into public builtin candidates and Yoi dogfood/project-local material.
- Recorded the proposed builtin workflow source/provenance model: add `WorkflowSource::Builtin` or equivalent typed provenance, with source labels visible in workflow/role launch diagnostics.
- Recorded source priority: explicit path/file selector, workspace `.yoi/workflow/<slug>.md`, future user workflow directory, builtin `resources/workflows/<slug>.md`, then explicitly enabled skill/plugin workflow sources.
- Chose `resources/workflows/<slug>.md` rather than `resources/prompts` for builtin workflow resources because workflows need slug/source/provenance/listing semantics distinct from prompt fragments.
- Recorded resident policy: core builtin resident workflows are intake and orchestrator routing; multi-agent is builtin-available but optional; Git worktree isolation remains project-local/default-disabled.
- Recorded slug/migration direction for `ticket-intake-workflow`, `ticket-orchestrator-routing`, `ticket-preflight-workflow`, `multi-agent-workflow`, and `worktree-workflow`.
- Recorded stale vocabulary cleanup plan for `Action required`, `Attention required`, and preflight lane language.
- Added a short pointer from `docs/development/workflows.md` to the design/audit document.

Validation run in branch:
- `git diff --check` passed.
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor` passed.

Nix build:
- Skipped because this branch is docs-only and does not change code, packaging, runtime resources, or prompt resources.

Notes:
- This Ticket intentionally does not implement builtin workflow loading or move workflow text into runtime resources. The document defines follow-up implementation boundaries for loader/provenance, public workflow text cleanup, dogfood retention, vocabulary migration, and launch provenance display.

---

<!-- event: review author: reviewer-workflow-public-dogfood-split at: 2026-06-11T02:30:27Z status: request_changes -->

## Review: request changes

Branch-local review verdict: request_changes

Blocker:
- Slug/migration plan is not concrete enough for the Ticket acceptance criteria. `docs/design/workflows-public-dogfood-split.md` leaves the key choice open as “overrides or dogfood-only slugs” and “Keep or rename workspace-local dogfood workflows”. The Ticket asks to decide whether dogfood workflows keep same-slug workspace overrides or move to explicit `yoi-dogfood-*`-style slugs, with rationale.
- Reviewer requested an explicit mapping for current selectors in `.yoi/ticket.config.toml`: intake, orchestrator, coder, reviewer.
- Reviewer specifically requested clarification for how generic builtin `multi-agent-workflow` and the Yoi dogfood multi-agent/merge/worktree extension will be selected without accidental shadowing.

Non-blocking notes:
- Diff is docs-only.
- The document audits all five current `.yoi/workflow/*.md` files.
- Source/provenance, source priority, workspace override vs builtin fallback, and `resources/workflows/<slug>.md` placement rationale are clear.
- Stale vocabulary cleanup and follow-up implementation boundaries are mostly clear, aside from the slug/config migration ambiguity.

Validation run by reviewer:
- `git diff --check $(git merge-base HEAD develop)..HEAD` passed.
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor` passed.
- Nix build not run; change is docs-only.

No files were modified by reviewer.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-11T02:31:54Z -->

## Implementation report

Fix-loop implementation report

Reviewer blocker fixed in amended implementation commit `21a25e12`:
- Added an explicit `Slug and .yoi/ticket.config.toml migration decision` section to `docs/design/workflows-public-dogfood-split.md`.
- Recorded that workflows with dogfood semantics must use explicit dogfood slugs rather than same-slug workspace overrides.
- Recorded concrete planned mapping:
  - `[roles.intake].workflow` -> `ticket-intake-workflow` (builtin core by default).
  - `[roles.orchestrator].workflow` -> `ticket-orchestrator-routing` (builtin core by default; dogfood mechanics supplied explicitly elsewhere).
  - `[roles.coder].workflow` -> `yoi-dogfood-multi-agent-workflow` (workspace-local dogfood workflow).
  - `[roles.reviewer].workflow` -> `yoi-dogfood-multi-agent-workflow` (workspace-local dogfood workflow).
  - `ticket-preflight-workflow` -> compatibility-only, non-resident.
  - `yoi-dogfood-worktree-workflow` -> workspace-local helper referenced by the dogfood multi-agent workflow.
- Clarified that `multi-agent-workflow` is reserved for the generic builtin role loop and should not be shadowed by this repository's dogfood workflow after builtin loading exists.

Validation after fix:
- `git diff --check` passed.
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor` passed.

Nix build remains skipped because the branch is docs-only and does not change code, packaging, runtime resources, prompt resources, or workflow resources.

---
