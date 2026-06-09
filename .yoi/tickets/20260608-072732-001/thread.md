<!-- event: create author: LocalTicketBackend at: 2026-06-08T07:27:32Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T07:28:29Z -->

## Decision

## Design consideration: Queue gate should account for unresolved dependencies

This relation metadata should be available before Orchestrator planning. That implies a stronger queue-time question:

If a Ticket has a project-level dependency such as `depends_on: X`, and `X` is still in `planning` / not ready / not resolved, should the user be prevented or warned before Queueing the dependent Ticket?

This needs explicit design before implementation.

Considerations:

- A dependency that is still `planning` likely means the dependent Ticket is not truly runnable yet.
- Queue should probably reject or strongly warn when unresolved `depends_on` / blocking relations remain.
- The behavior may differ by relation kind:
  - `depends_on` unresolved: likely hard block or confirmation-required;
  - `blocks`: informational for the blocked Ticket;
  - `related`: never blocks;
  - `parent` / `child`: depends on workflow semantics;
  - `supersedes` / `duplicate_of`: may redirect or discourage Queue.
- The Panel should be able to show why Queue is unavailable or risky before involving the Orchestrator.
- CLI Queue/state transitions should enforce the same rule or at least expose the same diagnostics.
- Orchestrator should still re-check relations at acceptance time, because relations may change between Queue and routing.

The implementation should decide whether unresolved dependencies make Queue invalid, require confirmation, or allow Queue with a visible blocked state. The important requirement is that dependency relation metadata is not only an Orchestrator input; it should also inform the human Queue gate.

---

<!-- event: decision author: hare at: 2026-06-08T13:22:42Z -->

## Decision

## Design consideration: avoid recreating umbrella Tickets through parent/child relations

Umbrella Tickets are being deprecated as long-lived broad progress containers. This affects relation design.

Parent/child relation kinds can easily recreate the umbrella pattern if they imply that one Ticket is a broad container whose close condition depends on manually inspecting child sufficiency. Therefore relation kinds such as `parent` / `child` should be reconsidered before implementation.

Design guidance:

- Keep concrete dependency/blocking relations distinct from broad grouping.
- Prefer `Objective` records for medium-term goal/strategy grouping.
- If decomposition needs to be represented, consider whether a relation such as `derived_from`, `split_from`, or `related` is safer than `parent` / `child`.
- Do not make relation metadata imply that a broad umbrella Ticket should remain open as a progress container.
- Any relation kind that approximates parent/child must define clear close semantics and avoid ambiguity around whether all child work is sufficient.

This Ticket should explicitly decide whether to include parent/child relations at all in the first version.

---

<!-- event: decision author: hare at: 2026-06-09T01:00:53Z -->

## Decision

## Decision: relation first version excludes hierarchy

The first version of typed Ticket relations must not include hierarchy/container relation kinds. Exclude:

- `parent` / `child`;
- `sub-ticket`;
- `umbrella`;
- `part_of` / `contains`.

This avoids recreating umbrella Tickets under another schema. The relation feature should focus on non-hierarchical direct relationships: dependency, blocking, related, supersedes, duplicate/replacement. Medium-term grouping belongs to Objective records, not Ticket hierarchy.

---

<!-- event: decision author: intake at: 2026-06-09T01:02:08Z -->

## Decision

## Intake refinement: ready にするための設計決定

この Ticket は新規 Ticket ではなく、既存 Ticket の refinement として扱う。Orchestrator が routing できるよう、未決定だった relation kind / inverse / queue gate の境界を次の通り固定する。

### Readiness

- readiness: implementation_ready
- risk_flags: [ticket-schema, workflow-state, panel, orchestrator-routing]

### Binding decisions / invariants

- first version の保存対象 relation kind は非階層 relation のみに限定する。
  - `depends_on`: この Ticket が対象 Ticket の完了/解決を必要とする。
  - `blocks`: この Ticket が対象 Ticket の実行/完了を妨げる。
  - `related`: 非 blocking の関連。
  - `supersedes`: この Ticket が対象 Ticket を置き換える。
  - `duplicate_of`: この Ticket が対象 Ticket の duplicate である。
- `blocked_by` / `superseded_by` / duplicate 逆一覧などの inverse view は保存済み forward relation から導出する。first version では inverse relation kind を別途保存して整合性を二重管理しない。
- `parent` / `child` / `sub-ticket` / `umbrella` / `part_of` / `contains` は first version で扱わない。Ticket relation metadata で umbrella Ticket を再作成しない。
- relation metadata は project-level Ticket record であり、Pod/session claim、worktree、branch ownership、coder/reviewer assignment、capacity、current planned order、`do_not_parallelize` を保存しない。これらは local role session registry / OrchestrationPlan / Pod metadata の責務に残す。
- Queue gate は unresolved blocking relation を無視してはならない。
  - 対象 Ticket の unresolved `depends_on` と incoming unresolved `blocks` は、Panel/CLI の `ready -> queued` 操作で visible reason 付きの block/diagnostic にする。
  - `related` は queue を block しない。
  - `supersedes` / `duplicate_of` は replacement/duplicate として visible diagnostic にする。自動的に全 relation を blocking 扱いしない。
  - Orchestrator は `queued -> inprogress` acceptance 時に relation を再確認する。Queue gate の判定は Orchestrator の再確認を省略する根拠ではない。

### Implementation latitude

- durable storage の具体形は、typed backend が検証でき、git-trackable で、`TicketShow` / `ticket doctor` / Panel summary から読める限り実装側で選んでよい。例: `item.md` frontmatter の typed field、または Ticket artifacts 配下の typed record。
- CLI の具体的な relation 操作名・オプション名、Panel 上の短い表示文言、cycle 検出の bounded な実装方式は実装側で選んでよい。
- `depends_on` と `blocks` は利用者が自然な方向で記録できる relation kind として両方残すが、表示・validation では inverse view を導出して重複/矛盾を診断してよい。

### Escalation conditions

- hierarchy/container semantics を追加したくなった場合。
- inverse relation を保存しないと実装できない設計にしたくなった場合。
- queue gate を unresolved dependency でも通常 queue 可能にしたくなった場合。
- relation metadata に Pod/session/worktree/assignment/capacity/current-batch order を保存したくなった場合。
- public Ticket schema または role workflow guidance で relation kind 名を上記から変更したくなった場合。

### Validation focus

- `ticket doctor` が dangling reference、invalid kind、self relation、obvious dependency/blocking cycle を bounded diagnostic として検出すること。
- `TicketShow` が forward relation と derived inverse view を理解できる形で表示すること。
- Panel/CLI が unresolved dependency/blocker を queue 前に visible にすること。
- Intake/Planning と Orchestrator guidance が project-level relation metadata と OrchestrationPlan runtime decision を混同しないこと。

---

<!-- event: intake_summary author: intake at: 2026-06-09T01:02:17Z -->

## Intake summary

既存 Ticket を refinement し、typed Ticket relation metadata の first version を非階層 relation に限定して implementation_ready にした。保存 relation kind は `depends_on` / `blocks` / `related` / `supersedes` / `duplicate_of`、inverse view は forward relation から導出し、`parent` / `child` / `sub-ticket` / `umbrella` / `part_of` / `contains` は扱わない。unresolved `depends_on` と incoming unresolved `blocks` は Panel/CLI queue gate で visible block/diagnostic にし、Orchestrator は acceptance 時に再確認する。runtime claims や OrchestrationPlan execution decisions は relation metadata に保存しない。blocking open questions はない。

---

<!-- event: state_changed author: intake at: 2026-06-09T01:02:17Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement により、relation kind、inverse relation の扱い、queue gate 境界、binding invariants、implementation latitude、escalation conditions が揃ったため ready に遷移する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T02:27:31Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T02:28:14Z -->

## Decision

Routing decision: defer_pending_ticket_identity_migration

This Ticket is queued and appears implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- `simplify-ticket-identity-fields` is already active, with Coder Pod `coder-simplify-ticket-identity-fields` running in `.worktree/simplify-ticket-identity-fields`.
- This Ticket depends on the same Ticket backend/schema/API/panel/doctor surfaces and explicitly needs canonical Ticket references.
- Implementing typed relations before the identity simplification lands would risk building relation storage and validation on soon-to-change identity/layout/state semantics.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- After `simplify-ticket-identity-fields` is merged/closed/cleaned up or explicitly held back, re-read this Ticket against the new identity model and accept it from queued if still unblocked.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T05:42:43Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Accepted queued implementation after identity simplification landed, was closed, and the workspace was verified clean on the flat Ticket layout. This Ticket is the lower-level project-level Ticket relation/reference surface, so it should run before Objective records, which may reference Tickets and relation concepts.

---

<!-- event: decision author: orchestrator at: 2026-06-09T05:42:43Z -->

## Decision

Routing decision: implementation_ready

Evidence checked:
- Ticket body/thread, including Intake refinement and explicit first-version relation decisions.
- Current flat Ticket identity state after `simplify-ticket-identity-fields` landed and closed.
- Current queued set: `typed-ticket-relation-metadata` and `objective-records-for-medium-term-goals`.
- Orchestration plan records: none existed before this routing; I recorded that typed relations should run before Objective records.
- Workspace/worktree state: main workspace clean enough to route, no active implementation worktree.

Reason:
- The Ticket has concrete binding decisions: non-hierarchical relation kinds only, forward stored relations with derived inverse views, queue-gate diagnostics for unresolved dependencies/blockers, and clear separation from OrchestrationPlan/runtime claims.
- It is lower-level than Objective records because Objective links should target stable canonical Ticket IDs and relation/reference semantics.
- No concrete missing decision remains that requires returning to planning.

IntentPacket:

Intent:
- Add typed, durable, non-hierarchical project-level Ticket-to-Ticket relation metadata on top of the flat canonical Ticket ID/state model.

Binding decisions / invariants:
- Supported stored relation kinds for first version: `depends_on`, `blocks`, `related`, `supersedes`, `duplicate_of`.
- Do not support hierarchy/container kinds: no `parent`, `child`, `sub-ticket`, `umbrella`, `part_of`, or `contains`.
- Store forward relations only; derive inverse views such as blocked-by / superseded-by / duplicate inverse from forward records.
- Relations are project-level Ticket metadata and must not store Pod/session claims, worktree paths, branch ownership, coder/reviewer assignments, capacity, current planned order, or `do_not_parallelize` execution-plan state.
- Store relations durably under the Ticket backend in a typed, git-trackable, backend-validated form.
- Use canonical opaque Ticket IDs only. Do not use title/slug words as relation authority.
- `TicketShow` must display relation metadata including useful inverse view.
- `ticket doctor` must validate dangling references, invalid kinds, self relations, and obvious dependency/blocking cycles with bounded diagnostics.
- Panel/CLI queue gate should not ignore unresolved blocking relations: unresolved `depends_on` and incoming unresolved `blocks` must be visible as block/diagnostic before `ready -> queued` and rechecked by Orchestrator at acceptance.
- `related` does not block queue; `supersedes` / `duplicate_of` produce visible replacement/duplicate diagnostics but are not automatically all blocking.
- Orchestrator routing treats relations as input constraints distinct from OrchestrationPlan runtime decisions.
- Do not implement Objective records here.
- Do not implement a full scheduler/dependency graph solver.
- Do not change Pod protocol/history/session semantics.

Implementation latitude:
- Coder may choose typed artifact vs current Ticket metadata field if it is validated, git-trackable, queryable, and works with flat IDs.
- CLI/tool names and exact Panel hint wording are implementation latitude if acceptance criteria are met.
- Cycle detection may be bounded and documented/test-covered.
- If the queue gate cannot be fully enforced in Panel/CLI without broad UI redesign, implement visible diagnostics and escalate the exact remaining gap rather than silently skipping it.

Reviewer focus:
- Verify relation storage is typed/validated and does not rely only on Markdown thread prose.
- Verify canonical ID references and local record migration/test fixtures use flat IDs.
- Verify no hierarchy/container relation kind slips in as current behavior.
- Verify OrchestrationPlan and Ticket relations remain distinct.
- Verify doctor and queue/panel/list/show surfaces expose unresolved dependency/blocker information before Orchestrator starts work.

Validation:
- Focused Ticket backend/parser/writer/doctor tests for relations, dangling references, invalid kinds, self relation, cycles, derived inverse views.
- Focused CLI/tool/schema tests for relation create/show/list/query or equivalent surfaces.
- Focused panel/queue-gate tests for unresolved dependency/blocker visibility if touched.
- Workflow/prompt/doc checks for Intake/Planning/Orchestrator relation guidance.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- `cargo check --workspace`.
- `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T05:43:42Z -->

## Implementation report

Implementation routing started.

Worktree/branch:
- Worktree: `.worktree/typed-ticket-relation-metadata`
- Branch: `typed-ticket-relation-metadata`
- Base/routing commit: `41a61d7 ticket: route typed relation metadata`

Spawned sibling Coder Pod:
- `coder-typed-ticket-relation-metadata`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Routing context:
- `simplify-ticket-identity-fields` is merged, closed, cleaned up, and the flat Ticket layout is active.
- `objective-records-for-medium-term-goals` remains queued behind this work; durable orchestration plan records now encode that ordering.
- This Ticket is the active lower-level Ticket relation/reference implementation work.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T06:19:32Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-typed-ticket-relation-metadata`
- Commit: `4601ad2b41d6c51c9d445ca2291726a658c42ab5 ticket: add typed relation metadata`
- Worktree status before review: clean branch `typed-ticket-relation-metadata`
- Stopped after collecting output to reclaim delegated worktree scope.

Selected design:
- Store forward project-level Ticket relation metadata per Ticket as typed JSON at `.yoi/tickets/<ticket-id>/artifacts/relations.json`.
- Stored relation kinds: `depends_on`, `blocks`, `related`, `supersedes`, `duplicate_of`.
- Derived inverse views are computed, not stored: `blocked_by`, `dependency_of`, `superseded_by`, `duplicated_by`.
- Relation authority uses canonical opaque Ticket IDs only.
- Unresolved `depends_on` and incoming unresolved `blocks` are treated as queue/acceptance blockers; `related` does not block; `supersedes` / `duplicate_of` are visible diagnostics only.

Implementation summary:
- Added typed relation structs, artifact read/write, backend relation query/view, and doctor validation in `crates/ticket`.
- Added `TicketRelationRecord` / `TicketRelationQuery` tools and relation JSON in TicketShow/List-related output.
- Added CLI relation commands: `yoi ticket relation add ...` and `yoi ticket relation list ...`.
- Updated workspace panel to display unresolved relation blockers for ready Tickets.
- Updated Orchestrator/work-item guidance to distinguish Ticket relations from OrchestrationPlan execution records.

Changed files:
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `crates/tui/src/workspace_panel.rs`
- `crates/yoi/src/ticket_cli.rs`
- `docs/development/work-items.md`

Coder validation reported passed:
- `cargo test -q -p ticket ticket_relations`
- `cargo test -q -p ticket queue_gate_rejects`
- `cargo test -q -p ticket doctor_validates_ticket_relations`
- `cargo test -q -p ticket ticket_relation_tools_record`
- `cargo test -q -p ticket ticket_tool_name_partitions_are_explicit`
- `cargo test -q -p yoi ticket_cli_records_lists_and_shows_relations`
- `cargo test -q -p tui workspace_panel_marks_ready_ticket_with_unresolved_relation_blocked`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`

Residual risks noted by coder:
- No relation delete/update operation in this first version.
- Derived inverse view scans relation artifacts; acceptable for current local scale but may need index/cache if Ticket counts grow.
- Cycle detection is bounded and intentionally not a scheduler/dependency solver.
- `supersedes` / `duplicate_of` do not automatically close/block/transition Tickets.

---

<!-- event: review author: reviewer at: 2026-06-09T06:28:28Z status: approve -->

## Review: approve

Approved review for commit `4601ad2b41d6c51c9d445ca2291726a658c42ab5`.

Evidence checked:
- `TicketRelationKind` stores only `depends_on`, `blocks`, `related`, `supersedes`, and `duplicate_of`; hierarchy/container names are not supported as current relation kinds.
- Relation storage is a typed `artifacts/relations.json` artifact with versioned JSON, `deny_unknown_fields`, owner-ticket validation, self/duplicate validation, and canonical Ticket-id lookup for targets.
- Inverse relation views are derived through `TicketShow`/CLI output; only forward records are written.
- Queue gates reject unresolved outgoing `depends_on` and incoming unresolved `blocks` at both `ready -> queued` and `queued -> inprogress`; Panel rows use the derived blocker view rather than silently offering Queue.
- `TicketRelationQuery` is read-only and `TicketRelationRecord` is mutating in the ticket tool partitions.
- Orchestrator guidance explicitly separates project-level relation metadata from OrchestrationPlan runtime ordering/capacity records.

Validation run from the worktree:
- `cargo test -q -p ticket ticket_relations`
- `cargo test -q -p ticket queue_gate_rejects`
- `cargo test -q -p ticket doctor_validates_ticket_relations`
- `cargo test -q -p ticket ticket_relation_tools_record`
- `cargo test -q -p ticket ticket_tool_name_partitions_are_explicit`
- `cargo test -q -p yoi ticket_cli_records_lists_and_shows_relations`
- `cargo test -q -p tui workspace_panel_marks_ready_ticket_with_unresolved_relation_blocked`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Residual risk: `TicketShow` tool description still summarizes the old output shape and does not mention relations, but the returned JSON and CLI display include relation metadata; this is documentation polish rather than a correctness blocker.

---

<!-- event: state_changed author: hare at: 2026-06-09T06:31:17Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-09T06:31:17Z status: closed -->

## 完了

Implemented, reviewed, merged, and validated.

Summary:
- Added typed, durable, non-hierarchical Ticket relation metadata.
- Stored forward project-level relations as typed JSON artifacts at `.yoi/tickets/<ticket-id>/artifacts/relations.json`.
- Supported first-version relation kinds: `depends_on`, `blocks`, `related`, `supersedes`, and `duplicate_of`.
- Derived inverse views such as `blocked_by`, `dependency_of`, `superseded_by`, and `duplicated_by` from forward relations rather than storing inverse relation kinds.
- Added Ticket backend validation and `ticket doctor` diagnostics for relation artifacts, dangling references, self relations, duplicate relations, and bounded dependency/blocking cycles.
- Added TicketRelation LLM tools, CLI relation add/list surfaces, TicketShow/List relation metadata, and workspace panel unresolved blocker hints.
- Kept relation metadata distinct from OrchestrationPlan execution records and from Pod/session/worktree/runtime claims.

Implementation:
- Coder commit: `4601ad2 ticket: add typed relation metadata`
- Reviewer approved with no blocking findings.
- Merge commit: `2225311 merge: add typed ticket relation metadata`

Validation after merge:
- `cargo test -q -p ticket ticket_relations`
- `cargo test -q -p ticket queue_gate_rejects`
- `cargo test -q -p ticket doctor_validates_ticket_relations`
- `cargo test -q -p ticket ticket_relation_tools_record`
- `cargo test -q -p ticket ticket_tool_name_partitions_are_explicit`
- `cargo test -q -p yoi ticket_cli_records_lists_and_shows_relations`
- `cargo test -q -p tui workspace_panel_marks_ready_ticket_with_unresolved_relation_blocked`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`


---
