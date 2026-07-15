<!-- event: create author: "yoi ticket" at: 2026-07-15T19:02:13Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-15T19:27:11Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-15T19:27:11Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-15T19:54:36Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-15T19:55:37Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は前回の broad Skills support scope から修正され、今回は Workflow tracking / Workflow resource / Workflow invocation path の削除に範囲が限定されている。
- Skills support は incoming dependent Ticket `00001KXKMX0QM` が `depends_on` として後続に分離されており、この Ticket 自身の blocker ではない。
- `TicketRelationQuery(00001KXKJGYGD)` は incoming dependency のみで、outgoing blocker はない。
- `TicketOrchestrationPlanQuery(00001KXKJGYGD)` は事前 record なし。今回 accepted plan を記録した。
- `TicketList(inprogress)` は 0 件。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` は clean で、既存 implementation worktree/branch は見当たらない。

Evidence checked:
- Ticket body / thread / relations / artifacts。
- `TicketRelationQuery`, `TicketOrchestrationPlanQuery`, `TicketList(inprogress)`。
- Orchestrator worktree / branch / worktree state。
- Previous broad scope was corrected: current title is `Remove workflow tracking and workflow resources` and body says Agent Skills is separate follow-up。

IntentPacket:

Intent:
- Legacy Workflow tracking/resource/invocation machinery を削除する。
- ActiveWorkflow obligation state と model-visible ActiveWorkflow tools を取り除く。
- Workflow resource authority と `/workflow-slug` invocation を廃止し、prompt/docs から Workflow 固有 wording を削除する。

Binding decisions / invariants:
- この Ticket では first-class Skills support を実装しない。Skills は dependent follow-up Ticket `00001KXKMX0QM` の責務。
- Workflow を scheduler / state machine / script / external-state automation layer として残さない。
- `ActiveWorkflowList` / `ActiveWorkflowComplete` / `ActiveWorkflowCancel` は model-visible tool schema から削除する。
- Worker durable state / snapshot / compaction / rehydration / system-item extension から active workflow snapshot / obligation を削除する。
- `resources/workflows` / `.yoi/workflow` は長期 authority として扱わない。
- Ticket / Worker / workdir / queue などの外部状態制御は typed feature/tool surface に残す。
- 既存 session に ActiveWorkflow storage がある場合は、初期実装では ignore / drop / diagnostic の bounded 方針にし、過剰な migration layer を作らない。

Requirements / acceptance criteria:
- ActiveWorkflow tools が schema から消えている。
- active workflow state が Worker snapshot / compaction / rehydration に残らない。
- Workflow resource discovery / invocation path が削除されている。
- Workflow prompt wording / resident workflow advertisement が消えている。
- Workflow 関連 crate/resource/API/test が削除または不要な互換なしに整理されている。
- External state control は feature/tool surface に残る。
- affected tests と `nix build .#yoi` が通る。

Implementation latitude:
- 既存 Workflow crate/resource を削除するか、必要最小限の型削除/rename にするかは dependency graph に応じて選んでよい。
- Persisted old ActiveWorkflow state は bounded compatibility として無視/drop/diagnostic のどれかを選び、設計が大きくなる場合は escalation。
- Prompt/docs cleanup の具体的な文言は Workflow authority を広告しない範囲で local tactic とする。

Escalate if:
- ActiveWorkflow state removal が destructive session migration や broad compatibility layer を必要とする場合。
- Skills support を同時実装しないと build/API が成立しない場合。
- `/slug` など user-facing invocation の代替設計をこの Ticket 内で固定する必要が出た場合。
- Workflow に external-state authority を残さないと既存 feature が動かない場合。

Validation:
- `rg "ActiveWorkflow|active_workflow|Active workflow"` with remaining hits explained or removed。
- `rg "resources/workflows|\.yoi/workflow|workflow invocation|Resident workflows"` with remaining hits explained or removed。
- `git diff --check`
- affected crate tests such as `cargo test -p worker --lib --tests`, `cargo test -p session-store --lib --tests`, `cargo test -p yoi --tests` if CLI/input touched, and workflow crate tests if crate remains。
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map / likely touch points:
- `crates/worker/src/active_workflow.rs`, `crates/worker/src/workflow`, `crates/worker/src/controller.rs`, `crates/worker/src/worker.rs`, `crates/worker/src/prompt/*`。
- `crates/workflow/*`, `resources/workflows/*`, `resources/prompts/*`。
- `crates/protocol/src/lib.rs`, `crates/session-store/src/system_item.rs`。
- CLI/TUI input path for `/slug` workflow invocation if present。
- docs mentioning Workflow / ActiveWorkflow / resident workflows。

Critical risks / reviewer focus:
- ActiveWorkflow tools/state still present under renamed or hidden paths。
- Workflow resource discovery/invocation surviving as hidden active path。
- Prompt/docs still advertising Workflow obligations or resident workflows。
- Accidentally implementing broad Skills support in this Ticket instead of leaving it to dependent follow-up。
- Ticket queue / Worker spawn / workdir management logic being moved into Workflow/Skill rather than remaining typed tools/features。

Next action:
- Record `queued -> inprogress`, create dedicated implementation worktree, and route to sibling Coder Pod。

---

<!-- event: state_changed author: orchestrator at: 2026-07-15T19:55:42Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Dashboard queue authorization was inspected, no blockers were found, and an accepted plan was recorded. Moving queued Ticket to inprogress before creating worktree or spawning role Pods.

---

<!-- event: implementation_report author: hare at: 2026-07-15T20:45:12Z -->

## Implementation report

Implemented Workflow removal.

Summary:
- Removed model-visible ActiveWorkflow tools and the Worker active-workflow store/extension/compaction/rehydration paths.
- Removed Workflow resource loading/invocation/completions and deleted the workflow crate plus tracked resources/workflows files.
- Simplified protocol/TUI/web segment handling by removing WorkflowInvoke and slash workflow completion behavior.
- Removed .yoi/workflow memory/workspace authority and workflow usage source handling.
- Removed Workflow/resident-workflow prompt/resource wording and updated Ticket role launch/config code to use typed role launch prompts without workflow slugs.
- Preserved bounded legacy session behavior by deserializing old kind:"workflow" SystemItem entries into a non-authority LegacyIgnored item that replays only an ignore diagnostic.
- Did not implement first-class Skills support.

Validation:
- rg "ActiveWorkflow|active_workflow|Active workflow" -> no matches (exit 1)
- rg "resources/workflows|\\.yoi/workflow|workflow invocation|Resident workflows" -> no matches (exit 1)
- git diff --check -> exit 0
- cargo test -p session-store --lib --tests -> exit 0
- cargo test -p worker --lib --tests -> exit 0
- cargo test -p yoi --tests -> exit 0
- cargo check -p yoi -> exit 0
- yoi ticket doctor -> exit 0
- nix build .#yoi --no-link -> exit 0


---

<!-- event: implementation_report author: hare at: 2026-07-15T21:04:19Z -->

## Implementation report

Follow-up fixes for external review blockers.

Summary:
- Removed stale web `workflow_invoke` protocol/model/composer/test behavior; slash-prefixed text now remains plain text in the web console composer.
- Updated the generated TypeScript protocol surface to match the Rust protocol without WorkflowInvoke.
- Removed remaining role workflow scaffold test expectation.
- Replaced the stale TUI live workflow SystemItem rendering test with a legacy workflow SystemItem ignore-path test.
- Removed stale docs/config wording that advertised loading Skills as Workflows.
- Stabilized the Worker discovery restore-planning test by sharing the runtime-dir sandbox used by allocation tests, so the required full worker test command is not affected by global runtime-dir test races.

Validation:
- rg "ActiveWorkflow|active_workflow|Active workflow" -> no matches (exit 1)
- rg "resources/workflows|\\.yoi/workflow|workflow invocation|Resident workflows|workflow_invoke" -> no matches (exit 1)
- git diff --check -> exit 0
- cargo test -p ticket --lib scaffold_config_includes_backend_and_all_fixed_roles -- --nocapture -> exit 0
- cargo test -p tui --lib live_legacy_workflow_system_item_is_ignored -- --nocapture -> exit 0
- cargo test -p session-store --lib --tests -> exit 0
- cargo test -p worker --lib --tests -> exit 0
- cargo test -p yoi --tests -> exit 0
- cd web/workspace && deno task check && deno task test -> exit 0
- cargo check -p yoi -> exit 0
- yoi ticket doctor -> exit 0
- nix build .#yoi --no-link -> exit 0


---

<!-- event: implementation_report author: hare at: 2026-07-15T21:22:40Z -->

## Implementation report

Follow-up fix for focused re-review blocker.

Summary:
- Removed `/` as a web console completion sigil; composer completions now only recognize commands, file refs, and Knowledge refs.
- Removed `workflow` from the web console completion result type and route-side completion response typing.
- Updated web completion tests so slash text remains plain text/no completion token.
- Removed stale Workflow completion/resolver wording from protocol and nearby comments.
- Did not add hidden Workflow compatibility or broad Skills replacement.

Validation:
- rg "ActiveWorkflow|active_workflow|Active workflow" -> no matches (exit 1)
- focused completion-surface rg for workflow_invoke / workflow completion kind / slash Workflow resolver wording / resources/workflows / .yoi/workflow / Resident workflows -> no matches (exit 1)
- focused web/protocol rg "kind.*workflow" -> no matches (exit 1); remaining project-wide "kind: workflow" mentions are only legacy persisted SystemItem ignore tests from the bounded compatibility path.
- git diff --check -> exit 0
- cd web/workspace && deno task check && deno task test -> exit 0
- cargo test -p yoi --tests -> exit 0
- cargo check -p yoi -> exit 0
- yoi ticket doctor -> exit 0
- nix build .#yoi --no-link -> exit 0


---
