<!-- event: create author: "yoi ticket" at: 2026-06-10T07:29:13Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-10T07:41:49Z -->

## Intake summary

既存 Ticket を確認し、`crates/client/src/ticket_role.rs` の `build_launch_prompt` / `TicketIntakeHandoff::append_prompt_lines` / Orchestrator・Coder・Reviewer execution guidance に LLM-facing prompt prose の直書きがあること、および `resources/prompts` 側に既存 prompt catalog があることを確認した。要件・受け入れ条件・非目標は実装判断に十分で、未決定の blocking question はない。readiness: implementation_ready。risk_flags: [prompt-context, runtime-resource, role-launch, override-semantics, workflow-boundary]。実装時は Ticket role launch の semantic を維持し、runtime 値の bounded/sectioned 埋め込みと user override 可能性を壊さず、残す hardcoded 文言が LLM prompt ではない理由を thread/implementation report に記録すること。関連参考: closed `00001KTCDHFPG`（Ticket role config）, closed `00001KSTRAW70`（prompt catalog を使った tool description）, planning `00001KSKBPAXR`（内部 Worker/Pod prompt の将来 Workflow 化）。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-10T07:41:49Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件同期済み。blocking open question はなく、Orchestrator が routing 可能な concrete work item として ready にする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-10T07:49:23Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-10T07:59:33Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: decision author: hare at: 2026-06-10T07:59:33Z -->

## Decision

Accepting implementation after workspace inspection. Expected overlap with TUI model setup wizard is low, so route in parallel in a dedicated worktree.


---

<!-- event: implementation_report author: hare at: 2026-06-10T08:11:58Z -->

## Implementation report

Implemented directly in worktree .worktree/prompt-resource-centralization because Pod-management tools are not available in this API session.\n\nBranch/commit:\n- prompt-resource-centralization / 342d5cf refactor: move ticket role prompts to resources\n\nSummary:\n- Moved Ticket role launch guidance strings from crates/client/src/ticket_role.rs into resources/prompts/ticket_role/*.md.\n- Added workspace/user override lookup for ticket_role prompt fragments before falling back to embedded resources.\n- Kept Rust code responsible for runtime composition, bounded dynamic fields, and segment assembly.\n- Added regression coverage for workspace prompt override behavior.\n\nValidation:\n- cargo fmt\n- cargo test -p client ticket_role --lib\n- git diff --check\n- cargo check -p client\n\nPending:\n- External review/merge validation not yet performed.\n- TUI model setup wizard worktree exists but implementation has not started.


---

<!-- event: plan author: orchestrator at: 2026-06-10T09:00:04Z -->

## Plan

Routing update: review_needed

Reason:
- Implementation report exists for branch `prompt-resource-centralization` / commit `342d5cf8`, but independent review and merge validation are not yet recorded.

Evidence checked:
- Ticket body/thread/artifacts and relation/orchestration-plan records.
- Worktree status for `.worktree/prompt-resource-centralization` is clean and branch has commit `342d5cf8` over `develop`.
- Visible Pods showed no existing child reviewer.

Next action:
- Spawned branch-local reviewer Pod `reviewer-prompt-resource-centralization` with read-only scope over the worktree and this Ticket. Reviewer is asked for approve/request_changes, blockers, notes, and focused validation evidence.

Boundary:
- This is branch-local review evidence only; no main-branch Ticket approval, merge, close, or cleanup is recorded by this routing step.

---

<!-- event: review author: reviewer-prompt-resource-centralization at: 2026-06-10T09:06:45Z status: approve -->

## Review: approve

Branch-local review verdict: approve

Evidence:
- Reviewed branch diff from merge-base `76165b47d664cb02a79c16272fbb5f8377a693bb` against `develop`.
- Verified production `crates/client/src/ticket_role.rs` no longer hardcodes the large LLM-facing Ticket role launch / intake handoff / orchestrator / coder / reviewer prompt prose; prompt fragments now live under `resources/prompts/ticket_role/*.md`.
- Verified runtime values remain sectioned/bounded and resource lookup order is workspace `.yoi/prompts/ticket_role/*.md` -> user prompts dir -> builtin embedded resource.
- Verified existing semantics and tests cover representative builtin prompt generation and workspace override behavior.

Validation run by reviewer:
- `cargo test -p client ticket_role --lib` passed.
- `git diff --check $(git merge-base HEAD develop)..HEAD` passed.
- `nix build .#yoi` passed.

Non-blocking notes:
- Dedicated user-level prompt override test could be added later, but workspace override exercises the same loader path sufficiently for this change.
- Prompt text fragments remain in test assertions only; this is not production prompt hardcoding.

This is branch-local review evidence; final main-branch approval/close still belongs to merge-completion.

---
