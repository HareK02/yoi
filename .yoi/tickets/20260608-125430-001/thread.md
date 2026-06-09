<!-- event: create author: LocalTicketBackend at: 2026-06-08T12:54:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-09T00:17:06Z -->

## Plan

## Intake refinement

既存 Ticket は `Objective` record の目的・背景・要件・受け入れ条件をすでに含んでおり、新規 Ticket 作成ではなくこの Ticket の refinement で足りる。`objective` label の重複 Ticket は見つからず、関連する planning Ticket として `typed-ticket-relation-metadata` と `deprecate-umbrella-tickets` がある。

### Binding decisions / invariants

- `Objective` は Ticket / OrchestrationPlan / Pod session claim と別の first-class project record として扱う。
- Objective は medium-term goal、motivation、strategy / design direction、success criteria / exit conditions、decision context、linked Tickets を保持するための軽量 record であり、実装可能 work item である Ticket を置き換えない。
- Objective と Ticket の link は dependency / blocking relation ではない。Ticket が Objective に属していても、その Objective 内の他 Ticket すべてに依存することを意味しない。
- Objective context は Intake / Planning / Orchestrator の判断材料になるが、実装判断の authority は引き続き各 Ticket body/thread と明示的な Ticket relation / OrchestrationPlan record にある。
- 初期設計は Markdown-oriented かつ local file backend 前提に留め、roadmap scheduling、milestones、OKR、dependency graph solving、全 Ticket への Objective 必須化は範囲外にする。
- `Project` / `Initiative` ではなく `Objective` という用語を使う。

### Implementation latitude

- 具体的な record schema、`.yoi/` 配下の path、frontmatter fields、CLI/API 名、Panel 表示の段階分けは実装時の調査で提案してよい。
- 初回 scope は設計文書と最小実装方針の提示で足りる。必要であれば小さな local-file reader/writer/API skeleton を提案してよいが、full roadmap system に広げない。
- Ticket 側に Objective reference を置くか、Objective 側に linked Tickets を列挙するか、または両方をどう扱うかは、validation と git-trackable record と運用の単純さを基準に設計してよい。
- `deprecate-umbrella-tickets` と `typed-ticket-relation-metadata` の内容は関連 context として読むべきだが、それらの実装をこの Ticket に取り込まない。

### Escalation conditions

- Objective が dependency/blocking relation、OrchestrationPlan runtime plan、または local Pod/session/worktree claim を兼ね始める設計になりそうな場合は Orchestrator/人間判断へ戻す。
- Objective を全 Ticket に必須化する、または scheduling / roadmap / OKR system に拡張する必要が出た場合は scope creep として戻す。
- public CLI/API、storage migration、backward compatibility、Panel UX の binding decision が複数案で割れる場合は、設計案と trade-off を thread に記録して判断を戻す。
- Ticket body/thread を読まずに Objective context だけで実装判断できるような導線になりそうな場合は、Ticket authority boundary に反するため戻す。

### Readiness

- readiness: implementation_ready
- risk_flags: [project-record, ticket-model, workflow, persistence, cli-api, panel-ux]
- blocking open questions: none

Validation は既存 acceptance criteria の通り `target/debug/yoi ticket doctor` と `git diff --check` を含める。runtime resource / prompt / packaging / code に触れる実装になった場合は repository guidance に従って focused tests と `nix build .#yoi` も実施する。

---

<!-- event: intake_summary author: intake at: 2026-06-09T00:17:13Z -->

## Intake summary

既存 Ticket を新規作成せず refinement した。`Objective` は Ticket / OrchestrationPlan / Pod/session claim と別の lightweight project record とし、medium-term goal・motivation・strategy/design direction・success criteria/exit conditions・decision context・linked Tickets を保持する。Ticket を置き換えず、Objective link は dependency/blocking relation ではない。初期 scope は Markdown/local-file 前提の設計と最小実装方針に限定し、roadmap scheduling、milestones、OKR、dependency graph solving、全 Ticket への Objective 必須化は範囲外。blocking open questions はなく、Orchestrator が implementation/design work として routing できる。

---

<!-- event: state_changed author: intake at: 2026-06-09T00:17:13Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement により目的・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions が揃ったため、workflow_state を ready にする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T02:27:33Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T02:28:14Z -->

## Decision

Routing decision: defer_pending_ticket_identity_migration

This Ticket is queued and appears implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- `simplify-ticket-identity-fields` is already active, with Coder Pod `coder-simplify-ticket-identity-fields` running in `.worktree/simplify-ticket-identity-fields`.
- Objective record design/linking depends on stable Ticket identity and reference semantics, and the active Ticket is changing canonical Ticket identity, flat layout, state, and lookup surfaces.
- Implementing Objective link/reference guidance before the identity simplification lands would create avoidable conflict and may choose reference shapes that immediately need migration.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- After `simplify-ticket-identity-fields` is merged/closed/cleaned up or explicitly held back, re-read this Ticket against the new identity model and accept it from queued if still unblocked.

---

<!-- event: decision author: orchestrator at: 2026-06-09T05:42:43Z -->

## Decision

Routing decision: waiting_for_relation_metadata

`objective-records-for-medium-term-goals` remains queued, but implementation is intentionally ordered after `typed-ticket-relation-metadata`.

Reason:
- Objective records need stable Ticket link/reference semantics.
- `typed-ticket-relation-metadata` is the lower-level non-hierarchical Ticket relation/reference surface that should land first.
- Running both in parallel would create avoidable conflict in project-record guidance, Ticket link semantics, panel/CLI display, and Orchestrator/Intake wording.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- Complete and merge `typed-ticket-relation-metadata` first.
- Re-read Objective records against the resulting relation/reference model, then accept from queued if still unblocked.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T06:32:20Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Accepted queued implementation after `typed-ticket-relation-metadata` landed, was validated, closed, and cleaned up. Objective records can now be designed against the flat canonical Ticket ID model and the newly available non-hierarchical Ticket relation metadata surface.

---

<!-- event: decision author: orchestrator at: 2026-06-09T06:32:20Z -->

## Decision

Routing decision: implementation_ready

Evidence checked:
- Ticket body and Intake refinement.
- Prior deferral decisions for identity simplification and typed relation metadata.
- OrchestrationPlan record requiring this Ticket to run after `20260608-072732-001`.
- Current state: `simplify-ticket-identity-fields` and `typed-ticket-relation-metadata` are merged, validated, closed, and cleaned up.
- Current Ticket model: flat canonical Ticket IDs, `state` lifecycle, and typed non-hierarchical Ticket relations are available.

Reason:
- The previous blockers are resolved.
- The Ticket has explicit binding decisions and clear acceptance criteria.
- This can proceed as a lightweight Objective record design/minimal implementation without replacing Tickets, typed Ticket relations, OrchestrationPlan, or Pod/session claims.

IntentPacket:

Intent:
- Define and implement the first lightweight Objective record surface for medium-term goals, using local git-trackable project records and canonical Ticket IDs for links.

Binding decisions / invariants:
- Objective is a first-class project record distinct from Ticket, TicketRelation, OrchestrationPlan, and Pod/session claims.
- Objective captures medium-term goal, motivation/background, strategy/design direction, success criteria/exit conditions, decision context, linked Tickets, and current Objective lifecycle if useful.
- Objective does not replace Tickets. Tickets remain implementable work items and implementation authority remains each Ticket body/thread/artifacts plus explicit Ticket relations and OrchestrationPlan records.
- Objective-to-Ticket links are not dependency/blocking relations. A Ticket linked to an Objective does not depend on every other linked Ticket.
- Use canonical opaque Ticket IDs for Objective links; do not use title/slug words as link authority.
- Keep the first version lightweight and Markdown/local-file oriented under a clear `.yoi/` path.
- Avoid roadmap scheduling, milestones, OKRs, dependency graph solving, and making Objectives mandatory for every Ticket.
- Do not implement or alter typed Ticket relation metadata except by using the already-landed relation concepts in docs/guidance where relevant.
- Do not implement broad Panel UX if a minimal record/API/CLI surface is sufficient for this first version; note future UX follow-ups if needed.
- Orchestrator/Intake guidance should say Objective context is judgment/background context, not a substitute for reading the Ticket.

Implementation latitude:
- Coder may choose exact storage path, e.g. `.yoi/objectives/<objective-id>/item.md` plus artifacts/thread if useful, if it is simple, typed enough, and git-trackable.
- Coder may choose whether this first version is docs/design + minimal backend/CLI/tool skeleton or a fuller reader/writer/list/show implementation, but it must satisfy the acceptance criteria and remain bounded.
- If implementing code, add focused parser/writer/doctor/CLI tests; if only designing, update docs/workflows and record a concrete implementation path with validation.
- Prefer a path-derived opaque Objective ID model consistent with the new Ticket identity direction if IDs are introduced.

Escalate if:
- Objective starts to duplicate Ticket dependency/blocking relations, OrchestrationPlan execution decisions, or Pod/session runtime claims.
- The design requires a full roadmap/OKR/scheduling system.
- Objective links require changing Ticket relation metadata or Ticket identity semantics.
- Storage/API shape requires a broad project-record framework beyond the first Objective surface.

Validation:
- If code/storage is implemented: focused Objective parser/writer/list/show/doctor/CLI tests and Ticket-link validation tests.
- If docs/design only: focused text validation that Objective is distinct from Ticket/TicketRelation/OrchestrationPlan/Pod claims, Objective links are non-blocking/non-dependency, and Ticket body/thread remains authority.
- `cargo fmt --check` if code/tests touched.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- `cargo check --workspace` if code/tests touched.
- `nix build .#yoi` if resources/docs/code/package inputs are touched.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T06:33:20Z -->

## Implementation report

Implementation routing started after the previously required typed Ticket relation metadata surface landed.

Worktree/branch:
- Worktree: `.worktree/objective-records-for-medium-term-goals`
- Branch: `objective-records-for-medium-term-goals`
- Base/routing commit: `4b3be3f ticket: route objective records`

Spawned sibling Coder Pod:
- `coder-objective-records-for-medium-term-goals`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Routing context:
- `simplify-ticket-identity-fields` is complete and the flat canonical Ticket ID/state model is active.
- `typed-ticket-relation-metadata` is complete, so Objective links can now be designed against stable non-hierarchical Ticket relation/reference semantics.
- This is now the active implementation work for the deferred Objective record Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T06:49:43Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-objective-records-for-medium-term-goals`
- Commit: `be1207254708bea4035235b4cc49186817b83156 objective: add lightweight records`
- Worktree status before review: clean branch `objective-records-for-medium-term-goals`
- Stopped after collecting output to reclaim delegated worktree scope.

Selected design:
- Objective records live at `.yoi/objectives/<objective-id>/item.md`.
- Objective IDs are path-derived opaque canonical IDs; title/slug words are not authority.
- Objective frontmatter fields: `title`, `state: active|paused|done|archived`, `created_at`, `updated_at`, `linked_tickets`.
- Required Markdown sections: Goal, Motivation/background, Strategy/design direction, Success criteria/exit conditions, Decision context.
- Objective-to-Ticket links use canonical Ticket IDs and are non-blocking context links, not Ticket dependency/blocking/ordering relations.

Implementation summary:
- Added top-level `yoi objective` CLI with create/list/show/doctor commands.
- Objective create/doctor validate linked Tickets against the configured Ticket backend root.
- Objective doctor validates frontmatter, state, timestamps, required body sections, and linked Ticket existence.
- Updated Intake/Orchestrator/work-item guidance so Objective context remains distinct from Tickets, TicketRelation, OrchestrationPlan, and Pod/session claims.
- Updated Nix package hash for the changed Cargo dependency graph.

Changed files:
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `Cargo.lock`
- `crates/yoi/Cargo.toml`
- `crates/yoi/src/main.rs`
- `crates/yoi/src/objective_cli.rs`
- `docs/development/work-items.md`
- `package.nix`

Coder validation reported passed:
- `cargo test -p yoi objective_cli`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- objective doctor`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`

Residual risks noted by coder:
- Objective surface is intentionally minimal CLI/backend only; Panel UX, Objective thread/artifacts, roadmap/OKR/scheduling are not implemented.
- Objective show uses exact Objective ID lookup only; prefix/title query is intentionally omitted to preserve opaque ID authority.
- `linked_tickets` validates Ticket existence but does not write Ticket relation metadata, preserving the non-blocking boundary.

---
