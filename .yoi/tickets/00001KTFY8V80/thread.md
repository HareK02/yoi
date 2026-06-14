<!-- event: create author: LocalTicketBackend at: 2026-06-07T02:23:28Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: comment author: ticket-intake at: 2026-06-14T14:13:35Z -->

## Comment

## Intake refinement

既存 Ticket `00001KTFY8V80` を確認した。新規 duplicate Ticket は作成しない。

### Readiness

- readiness: implementation_ready
- risk_flags: [prompt-context, persistence, workflow-state, compaction]

この Ticket は、active workflow を compaction / rehydration 後も継続可能にする concrete work item として十分に bounded されている。実装戦術の調査余地は残るが、Orchestrator が implementation routing できる要件・受け入れ条件・検証観点は揃っている。

### Binding decisions / invariants

- active workflow の進行中状態を、history に残らない transient context 注入だけで復元してはならない。
- compaction / restore 後に「どの workflow が継続中か」「どの手順段階・義務が残っているか」をモデルが説明可能でなければならない。
- workflow state の復元は、prompt context 加工原則に反しない形で durable source から再構成する。
- missing / corrupt / obsolete workflow state は fail-closed または bounded diagnostic として扱い、silently stale instructions を実行しない。
- Ticket / Pod history / workflow record / compaction output の authority boundary を混同しない。

### Implementation latitude

- workflow state の永続化先・schema・snapshot 粒度は、既存 Pod/session/compaction architecture に合わせて選んでよい。
- active workflow body を invocation-time snapshot として保持するか、rehydration 時に最新 resource を参照するかは、実装時に明示的に決定し、互換性・安全性の理由をコードまたは docs / Ticket 報告に残す。
- UI/diagnostic 表示の具体的な文言や internal field 名は、既存設計に沿って調整してよい。

### Escalation conditions

- workflow snapshot vs latest body の選択が authority boundary または backward compatibility を大きく変える場合。
- compaction が workflow obligations を再現するために hidden context injection を必要としそうな場合。
- persisted workflow state の migration / compatibility 方針が既存 records を破壊する場合。
- implementation が Ticket lifecycle / Orchestrator queue semantics / workflow invocation semantics を広げる必要を見つけた場合。

### Related context checked

- closed `00001KTG3AZQ8` / `00001KTG3BX0R` は Orchestrator routing / merge completion の完了済み関連文脈であり、本 Ticket の duplicate ではない。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-14T14:13:43Z -->

## Intake summary

既存 Ticket `00001KTFY8V80` を精査し、duplicate は作成しない方針で refinement を記録した。対象は active Workflow invocation/state/obligations を durable state/history と compaction/rehydration 経路に載せ、compaction 後も `/multi-agent-workflow` / `/worktree-workflow` などの active obligations を traceable に継続できるようにする実装 work item。readiness は implementation_ready。risk flags は prompt-context / persistence / workflow-state / compaction。Orchestrator は implementation routing 可能だが、snapshot vs latest workflow body の選択、hidden context injection 回避、missing/corrupt persisted state の fail-closed diagnostic、Ticket/Pod/history/workflow authority boundary を reviewer focus に含める。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-14T14:13:43Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement が完了し、要件・受け入れ条件・binding invariants・escalation conditions が Ticket thread に記録されたため `planning -> ready` にします。実装 side effects は Orchestrator routing 後に行います。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:23:07Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:24:40Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- 要件、受け入れ条件、binding invariants、implementation latitude、escalation conditions が Ticket body/thread に揃っている。
- active Workflow invocation/state/obligations を durable history/state と compaction/rehydration 経路に載せる目的は concrete で、残る不確実性は既存 Pod/session/compaction architecture 内の実装戦術選択に閉じている。

Evidence checked:
- Ticket body / thread / artifacts: artifacts なし、Intake refinement と `planning -> ready`、Panel `ready -> queued` を確認。
- Ticket relations: blocking relation なし。
- OrchestrationPlan records: 既存 record なし。
- Orchestrator workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `d311fe8f` 上。
- Visible Pods: spawned child なし。
- Bounded code map: workflow / compaction 関連は `crates/pod/src/compact/*`, `crates/pod/src/workflow/*`, `crates/pod/src/prompt/*`, `crates/session-store/src/*`, `crates/protocol/src/lib.rs`, `resources/workflows/*` が候補。

IntentPacket:

Intent:
- compaction を跨ぐ長時間 workflow-governed task で、active workflow と残る operational obligations が失われないようにする。

Binding decisions / invariants:
- Workflow instructions を、history/state に残らない turn-local transient context だけを根拠に model context へ注入しない。
- post-compaction context は「available workflow」と「この task で active な workflow obligations」を区別する。
- missing / corrupt / obsolete active workflow state は silent stale instruction ではなく fail-closed または bounded diagnostic にする。
- Ticket / Pod history / workflow record / compaction output の authority boundary を混同しない。
- active workflow state は workflow-governed task の完了または explicit cancellation で clear / completed にできる必要がある。

Requirements / acceptance criteria:
- active workflow の slug、invocation source/time、task/scope、active/completed、current obligations/checkpoints を durable typed history/state として表現する。
- compaction が active workflow state を明示的に carry forward する。
- rehydration が durable source から active workflow guidance を復元できる。
- snapshot vs latest workflow body の選択を実装報告または docs/code に明示する。
- focused coverage に、review delegation と merge/close handling の間で compaction が起きる worktree/multi-agent style flow を含める。

Implementation latitude:
- 永続化先、schema、snapshot 粒度、diagnostic 表現は既存 Pod/session/compaction architecture に合わせて選んでよい。
- local tactic 調査は coder に委ねるが、authority boundary を広げる必要があれば escalate する。

Escalate if:
- workflow snapshot vs latest body の選択が authority boundary や backward compatibility を大きく変える。
- compaction 復元が hidden context injection を必要としそうになる。
- persisted workflow state migration / compatibility が既存 records を破壊しそうになる。
- Ticket lifecycle / Orchestrator queue semantics / workflow invocation semantics を広げる必要が出る。

Validation:
- 変更箇所に応じて `cargo test` / `cargo check` の focused subset。
- 少なくとも workflow/compaction 関連 unit coverage、`cargo fmt --check`、`git diff --check`。

Current code map:
- Primary candidates: `crates/pod/src/compact/*`, `crates/pod/src/workflow/*`, `crates/pod/src/prompt/*`, `crates/session-store/src/*`, `crates/protocol/src/lib.rs`。
- Workflow resources: `resources/workflows/*`。

Critical risks / reviewer focus:
- hidden context injection 回避。
- active vs advertised workflow の明確な区別。
- stale workflow obligations の漏れ込み防止。
- persisted state の compatibility / corrupt-state diagnostics。
- compaction 後の traceability と test coverage。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:24:58Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / orchestration-plan blocker はなく、Orchestrator workspace は clean。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KTFY8V80 at: 2026-06-14T15:50:38Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KTFY8V80`:

Commit:
- `362fedfb fix: preserve active workflows across compaction`

Changed files:
- `crates/pod/src/active_workflow.rs`
- `crates/pod/src/lib.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/segment_log_sink.rs`
- `resources/prompts/internal/compact_system.md`

Implemented behavior:
- Added durable typed active workflow state as session-log extension domain `pod.active_workflows`.
- State records include:
  - workflow slug
  - invocation source/time
  - task scope
  - active/completed/cancelled status
  - snapshotted workflow guidance
  - extracted obligations/checkpoints
  - completion/cancellation reason/time
- Workflow bodies are snapshotted at invocation time rather than resolved to latest resource/builtin version during rehydration. Rationale: active workflow authority remains traceable to the original governed task and does not silently change when resource files change later.
- Compaction now:
  - feeds active workflow state into compact worker input
  - writes active workflow state into the replacement segment as typed extension state
  - injects post-compaction workflow guidance into `SegmentStart.history` from durable state, not transient turn-local data
- Added `ActiveWorkflowList`, `ActiveWorkflowComplete`, and `ActiveWorkflowCancel` tools so active workflow state can be inspected, completed, or explicitly cancelled.
- Missing/corrupt/unsupported active workflow extension state fails closed with bounded diagnostics rather than reusing stale prior state.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `git diff --check`
- Passed: `cargo test -p pod active_workflow --lib`
  - includes focused coverage for review/merge/close-style obligations crossing compaction/rehydration
- Passed: `cargo test -p pod includes_active_workflow_snapshot_section --lib`
- Ran: `cargo test -p pod --lib`
  - Failed on 2 prompt text assertions reported as unrelated/pre-existing:
    - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
    - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
    - both assert the unrelated string `worktree status, diff, and test results`

Repository status:
- Child implementation worktree clean after commit.

Residual risks / notes:
- Active workflow obligation extraction is intentionally conservative: it stores full snapshotted guidance as authority and derives bounded checkpoint labels from obligation-like lines.
- Completion/cancellation tool calls persist through normal history; compaction additionally writes updated typed snapshot into the compacted segment.

---

<!-- event: review author: yoi-reviewer-00001KTFY8V80 at: 2026-06-14T15:58:49Z status: request_changes -->

## Review: request changes

Review result: request_changes

Evidence checked:
- Child worktree/branch/head:
  - `/home/hare/Projects/yoi/.worktree/00001KTFY8V80-active-workflows-compaction`
  - `impl/00001KTFY8V80-active-workflows-compaction`
  - HEAD `362fedfbe6689886f1e2e7c29da61e39b0ce1e38`
  - merge base with requested base: `73d0a6a4`
- `git status --short` was clean.
- Diff `73d0a6a4..362fedfb` inspected.
- Read-only validation:
  - Passed: `git diff --check 73d0a6a4..362fedfb`
- Cargo/fmt not rerun because review scope was read-only.

What looks good:
- A typed active workflow snapshot was added with slug, status, invocation source/time, task scope, snapshot policy, snapshotted guidance, obligations/checkpoints, and completion metadata.
- Active workflow state is separated from advertised workflows; activation comes from invoked `SystemItem::Workflow` rather than resident workflow catalog.
- Snapshot-vs-latest behavior is explicit via `WorkflowBodySnapshotPolicy::SnapshottedAtInvocation`.
- Compaction passes active workflow state into compactor input and writes typed `LogEntry::Extension` into the compacted segment.
- Clear/cancel tools are exposed as `ActiveWorkflowComplete` / `ActiveWorkflowCancel`.

Required changes:

1. Stale active workflow guidance can remain in prompt history after typed state is invalid, completed, or cancelled.

- The implementation writes active workflow rehydration guidance as an ordinary system message in compacted history (`pod.rs` around the compaction replacement history construction).
- Restore later uses `SegmentStart.history` as worker history.
- Corrupt/obsolete extension handling drops/diagnoses the typed state but does not remove the old `[Active workflow snapshot]` system message from compacted history.
- Therefore the model can still see stale workflow obligations even when the durable active-workflow extension is missing/corrupt/obsolete.
- The same leakage risk applies after completion/cancellation: old compacted system messages can remain until another compaction.

Required fix:
- Ensure active workflow guidance shown to the model is gated by currently valid active workflow state, not immutable old compacted history.
- For example, regenerate guidance from validated typed state at context/compaction time, or sanitize/supersede old active workflow system messages when typed state is invalid/inactive.
- Add focused coverage for compacted history containing an active workflow message plus missing/corrupt/obsolete extension, and for completion/cancellation after compaction.

2. Completion/cancellation durability is inferred from generic tool-call history and trusts bare `ToolCall`s.

- `ActiveWorkflowComplete` / `ActiveWorkflowCancel` mutate only the in-memory store in the tool implementation.
- They do not append a new typed `LogEntry::Extension` on successful status change.
- Restore reconstructs completion/cancellation by scanning history.
- Replay marks workflows completed/cancelled based solely on `Item::ToolCall` names, without requiring a matching successful `ToolResult`.

Required fix:
- Persist status changes as typed durable active workflow state when the status tool succeeds, or make replay validate a matching non-error tool result/event proving the tool executed successfully.
- Add focused tests for interrupted/unmatched/error status tool calls.

Prompt-test failure assessment:
- The reported `cargo test -p pod --lib` failures about `worktree status, diff, and test results` appear unrelated. The diff changes `resources/prompts/internal/compact_system.md`, while the failing assertions are in pod orchestration prompt tests and reference a different prompt resource.

Conclusion:
- Changes requested. Do not integrate until stale-guidance gating and completion/cancellation durability are fixed and covered.

---

<!-- event: implementation_report author: yoi-coder-00001KTFY8V80 at: 2026-06-14T16:17:48Z -->

## Implementation report

Follow-up implementation report after reviewer request_changes:

New commit:
- `ff446052 fix: gate active workflow rehydration state`

Changed files in follow-up:
- `crates/pod/src/active_workflow.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/pod.rs`

Review blocker 1 resolved: stale active workflow guidance in prompt history
- Active workflow rehydration guidance is no longer persisted into compacted `SegmentStart.history` as an ordinary system message.
- Compaction still carries active workflow state forward via typed `pod.active_workflows` extension state.
- Request context is sanitized before model calls:
  - stale `[Active workflow snapshot]` system messages are stripped,
  - current active workflow guidance is regenerated request-locally from validated typed state,
  - when typed state is missing/corrupt/obsolete/inactive and stale workflow hints are present, a bounded inactive-state message supersedes old compacted references.
- Restore paths strip stale active workflow rehydration messages before installing worker history.
- Compaction source/retained histories strip old active workflow rehydration messages so old snapshots are not re-summarized as active authority.

Review blocker 2 resolved: completion/cancellation durability
- `ActiveWorkflowComplete` / `ActiveWorkflowCancel` now persist successful status changes by committing a new typed `LogEntry::Extension` snapshot through the session log writer.
- Restore no longer infers completion/cancellation from bare `Item::ToolCall` history.
- Typed extension state is the authority, avoiding trust in unmatched/interrupted/error tool calls.

Focused coverage added/updated:
- Compacted history containing active workflow rehydration message plus:
  - missing extension,
  - corrupt extension,
  - obsolete extension.
- Completion/cancellation after compaction suppressing old compacted guidance.
- Successful status tool writes a typed durable extension.
- Unmatched/error status tool calls do not mutate restored active workflow state.
- Existing review/merge/close workflow obligation coverage still passes.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `git diff --check`
- Passed: `cargo test -p pod active_workflow --lib`
  - 7 passed
- Passed: `cargo test -p pod interceptor --lib`
  - 19 passed
- Ran: `cargo test -p pod --lib`
  - Still fails only on the previously reported unrelated prompt assertion failures:
    - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
    - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
    - both assert missing text: `worktree status, diff, and test results`
    - result: `332 passed; 2 failed`

Repository status:
- Child implementation worktree clean after follow-up commit.

---

<!-- event: review author: yoi-reviewer-00001KTFY8V80-r2 at: 2026-06-14T16:25:00Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Child worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KTFY8V80-active-workflows-compaction`
- HEAD: `ff446052 fix: gate active workflow rehydration state`
- Base checked against: `73d0a6a4`
- Original implementation checked where relevant: `362fedfb`

Prior blocker 1: stale active workflow guidance after compaction
- Resolved.
- Active workflow rehydration is now derived from typed `LogEntry::Extension` state via `ActiveWorkflowStore`, not from ordinary prompt/tool-call history.
- Restore strips prior `[Active workflow snapshot]` system messages before installing history.
- Rehydration guidance is regenerated request-time from validated active typed state.
- Missing/corrupt/unsupported extension state fails closed: no active workflow restored, stale rehydration messages stripped, and bounded inactive diagnostic text tells the model not to treat older compacted history/summaries as active workflow authority.
- Completed/cancelled typed state does not regenerate active guidance.
- Compaction no longer stores active workflow guidance directly in `SegmentStart.history` as ordinary durable prompt authority; it carries typed extension entries.
- Focused coverage exists for stale active workflow message plus missing/corrupt/unsupported state and completion/cancellation after compaction.

Prior blocker 2: completion/cancellation durability
- Resolved.
- `ActiveWorkflowComplete` / `ActiveWorkflowCancel` mutate store status and commit a fresh typed `LogEntry::Extension` snapshot through the active workflow log committer.
- Production controller wiring attaches the log writer before feature/tool registration, so status tools have durable commit plumbing.
- Restore no longer trusts bare unmatched `Item::ToolCall` entries or failed/error calls to infer completed/cancelled state.
- Focused tests cover unmatched/error status tool calls and explicit completed/cancelled typed extension suppression of active guidance.

Overall acceptance:
- Durable typed active workflow representation exists.
- Compaction carries active workflow state forward through typed extension state.
- Rehydration restores guidance from durable validated state.
- Snapshot-vs-latest policy is explicit and fail-closed on missing/corrupt/unsupported latest state.
- No hidden context injection from non-durable transient data was found.
- Active vs advertised workflow separation is preserved.
- Clear/cancel/complete behavior is durable typed-state transition.

Validation performed by reviewer:
- Passed: `git diff --check 73d0a6a4..HEAD`
- Passed: `cargo fmt --check`
- `git status --short` remained clean.

Validation not rerun by reviewer:
- Cargo tests were not rerun because review scope was read-only and tests write build artifacts. Coder-reported focused test results were inspected as evidence.

Full-suite prompt failure assessment:
- The remaining reported `cargo test -p pod --lib` prompt assertion failures involving `worktree status, diff, and test results` appear unrelated to the active workflow typed-state/compaction changes.

Conclusion:
- Approved. No remaining blocker found.

---
