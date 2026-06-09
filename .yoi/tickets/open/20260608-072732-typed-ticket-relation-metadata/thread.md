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
