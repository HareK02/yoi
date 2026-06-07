<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:52:31Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-07T05:00:22Z -->

## Decision

## Git/Ticket review record boundary

Decision:
- Reviewer verdicts on an unmerged implementation branch are branch-scoped evidence, not final main-branch Ticket approval.
- The Orchestrator should keep branch-local review results in the merge-ready dossier/review report during the worktree-agent phase.
- Main Ticket thread may still record durable progress such as worktree plan, coder/reviewer delegation, blockers, and fix-loop status.
- The final Ticket review/approval/completion record should be written during the merge-completion phase, after confirming the reviewed branch is the branch being merged, or as part of the same controlled completion sequence.

Rationale:
- Avoid main branch Ticket history claiming approval for code that is not yet in main.
- Keep git history aligned: implementation branch review evidence leads to merge; main Ticket lifecycle records the completed merge/validation outcome.
- Builtin Orchestrator workflow should make this explicit so automation does not reproduce the ad-hoc parent-side `TicketReview` timing used during manual dogfooding.

---

<!-- event: plan author: hare at: 2026-06-07T06:15:24Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready as the third Orchestrator automation slice, after queued routing and worktree-agent routing landed.

Intent:
- Add explicit Orchestrator merge/completion guidance or focused wiring for reviewed in-progress Tickets that have a merge-ready dossier.
- Preserve the git/Ticket boundary: branch-local reviewer verdicts are evidence in the dossier before merge; final main Ticket approval/completion records are written only during the controlled merge-completion phase after confirming the reviewed branch is the branch being merged.
- Support dogfooding authorization for Orchestrator merge/cleanup/close, while conservative/missing authorization stops at merge-ready dossier.

Prerequisite state:
- `orchestrator-queued-ticket-routing` landed: Queue notification authorizes routing and requires `queued -> inprogress` before side effects.
- `orchestrator-worktree-agent-routing` landed: accepted Tickets can be routed through worktree + coder/reviewer sibling guidance and stop at merge-ready dossier.
- Ticket lifecycle tools and role launch config strictness/scaffold have landed.

Requirements for this slice:
- Add merge-completion guidance to the relevant Orchestrator role prompt/workflow surface.
- Merge conditions:
  - Ticket is `inprogress` and has a merge-ready dossier;
  - reviewed branch/worktree/commits in the dossier match the branch to be merged;
  - independent reviewer approval exists in the dossier, or a human override decision is explicitly recorded;
  - main workspace is safe and unrelated dirty changes are understood;
  - merge authority is available in dogfooding/workspace policy, otherwise stop at dossier.
- Merge/completion sequence:
  - stop/reclaim coder/reviewer Pods where appropriate;
  - `git merge --no-ff <branch>` or project-agreed merge method;
  - run post-merge validation appropriate to the change;
  - record review/merge/validation outcome in the Ticket thread during merge-completion, not before;
  - transition `inprogress -> done` or close the Ticket according to typed Ticket workflow rules;
  - remove merged child worktree and delete merged branch unless explicitly kept.
- Add/update tests for merge authority boundary and required dossier/validation fields in generated guidance or helper text.

Likely implementation surfaces:
- `crates/client/src/ticket_role.rs`: existing Orchestrator role guidance already stops at merge-ready dossier; extend with merge-completion guidance for the authorized completion phase.
- `crates/tui/src/multi_pod.rs`: Queue notification should probably remain focused on routing/agent path and not over-expand into merge details unless existing tests require consistency.
- `.yoi/workflow/multi-agent-workflow.md` may need parent-side wording updates to align with branch-local verdict vs main Ticket approval boundary. Child worktree excludes `.yoi`, so any workflow edits must be reported for parent-side application.

Non-goals:
- Do not implement Queue notification/acceptance.
- Do not implement worktree/coder/reviewer routing.
- Do not remove human override path.
- Do not make merge authority implicit for all public/default configurations.
- Do not solve active workflow compaction persistence here.

Validation:
- Run focused tests for changed crates, especially `cargo test -p client ticket_role --lib` if role guidance changes.
- Run `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` if Ticket/workflow records are touched.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T06:15:32Z -->

## Intake summary

Implementation-ready as the third Orchestrator automation slice: add merge-completion guidance for reviewed in-progress Tickets with merge-ready dossiers, preserving branch-local review vs final main Ticket approval boundaries, dogfooding merge authorization, post-merge validation, Ticket completion, and worktree/branch/Pod cleanup.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T06:15:32Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T06:16:17Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `orchestrator-merge-coder-20260607` in worktree `.worktree/orchestrator-merge-completion` on branch `work/orchestrator-merge-completion`.

Scope:
- Add Orchestrator merge/completion guidance for reviewed in-progress Tickets with merge-ready dossiers.
- Preserve branch-local reviewer verdict vs final main Ticket approval boundary.
- Define dogfooding merge authority vs conservative stop-at-dossier behavior.
- Include post-merge validation, Ticket completion, worktree/branch/Pod cleanup guidance.
- Do not implement queue routing or worktree/coder/reviewer routing.

Workflow note:
- Child worktree excludes `.yoi`; if workflow file updates are needed, the child should report an exact parent-side patch rather than editing `.yoi`.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, workflow file updates, and cleanup remain with the parent/human workflow.

---

<!-- event: decision author: hare at: 2026-06-07T06:29:12Z -->

## Decision

## Related worktree/memory boundary follow-ups

Created two follow-up tickets from the `.yoi` sparse checkout / memory-root discussion:

- `narrow-yoi-worktree-sparse-exclusions`
  - Stop excluding all `.yoi/**` from child worktrees.
  - Include tracked Yoi project records such as tickets/workflows/config where appropriate.
  - Continue excluding generated memory, logs, locks, local/runtime/secret-like files.
  - Define child-vs-main Ticket edit policy for branch-local artifacts vs final orchestration records.

- `memory-root-uses-yoi-memory-marker`
  - Keep `.yoi` as the Yoi project records marker.
  - Stop treating `.yoi` alone as the memory root marker.
  - Use explicit memory config or nearest ancestor `.yoi/memory` as repo-local memory root.
  - Ensure child worktrees containing `.yoi/tickets` but not `.yoi/memory` do not become independent memory workspaces.

These are relevant to Orchestrator merge/worktree automation because branch-local dossiers and workflow edits should be possible without copying generated memory into child worktrees.

---

<!-- event: implementation_report author: hare at: 2026-06-07T06:38:16Z -->

## Implementation report

## Implementation report

Coder Pod `orchestrator-merge-coder-20260607` completed implementation in `.worktree/orchestrator-merge-completion`.

Commit:
- `cb17728 orchestrator: add merge completion guidance`

Observed diff:
- `crates/client/src/ticket_role.rs` only.

Implementation summary:
- Extended Orchestrator role guidance with merge-completion instructions for reviewed in-progress Tickets with merge-ready dossiers.
- Guidance requires the Ticket to be `inprogress` and the dossier branch/worktree/commits to match the branch being merged.
- Requires independent reviewer approval in the dossier or an explicit human override decision.
- Requires main workspace safety / unrelated dirty change awareness.
- Distinguishes dogfooding/workspace merge authority from conservative/missing authorization mode, which stops at the dossier.
- Preserves branch-local reviewer verdict vs final main Ticket approval boundary.
- Defines the authorized sequence: stop/reclaim coder/reviewer Pods where appropriate, merge, post-merge validation, record review/merge/validation outcome in Ticket thread, transition `inprogress -> done` or close, then cleanup worktree/branch unless explicitly kept.
- Added tests covering merge-completion guidance / dossier requirements.

Reported validation:
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `nix build .#yoi`

Parent-side workflow patch needed: none observed from the implementation; child diff is code/prompt-generation only.

External review will be delegated before merge.

---

<!-- event: review author: orchestrator-merge-reviewer-20260607 at: 2026-06-07T06:41:49Z status: request_changes -->

## Review: request changes

Request changes.

The implementation gets the merge authority and branch-review boundary right, but it does not yet satisfy the ticket's required dossier/validation specificity, and the added test does not cover those missing fields.

Positive evidence:
- Merge authority boundary is good: conservative/missing authorization stops at the dossier and merge authority is not inferred from public/default config.
- Branch-local reviewer verdict vs final main Ticket approval boundary is preserved.
- Queue routing and worktree/coder/reviewer routing were not reimplemented beyond guidance; child commit changes only `crates/client/src/ticket_role.rs`.

Blockers:
1. Merge-ready dossier fields are not explicit enough. The current text lists branch, worktree path, commits, implementation summary, validation evidence, coder/reviewer evidence, blocker loop outcome, residual risk, and parent decision needs, but the ticket explicitly requires Ticket id/slug, intent/invariant check, coder/reviewer Pods, blockers fixed or rejected findings with reasons, validation performed, and dirty state. Several are missing or only implied.

2. Post-merge validation baseline is too generic. The ticket requires explicit minimum baseline guidance: focused tests from Ticket/dossier, `cargo fmt --check`, `git diff --check`, `target/debug/yoi ticket doctor` where applicable, plus broader validation such as `cargo check --workspace --all-targets` / `nix build .#yoi` when risk or touched files warrant it.

3. Test coverage mirrors the same gap. The new test covers authority, branch/worktree/commit matching, reviewer approval/override, main workspace safety, branch-local boundary, merge command, cleanup, and generic validation evidence, but does not assert the missing dossier fields or required validation baseline commands.

Reviewer validation:
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check develop..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor` from main workspace
- `nix build .#yoi` (dirty-tree warning only; generated `result` symlink removed)
- merge-tree check against current `develop`

Merge readiness: not merge-ready until dossier/validation guidance and tests are explicit.

---

<!-- event: review author: orchestrator-merge-reviewer-20260607 at: 2026-06-07T06:46:06Z status: approve -->

## Review: approve

Approved after fixup commit `33abf3f`.

Evidence:
- Dossier fields are now explicit in `crates/client/src/ticket_role.rs`, including Ticket id/slug, branch/worktree, commits, intent/invariant check, implementation summary, coder/reviewer Pods, blockers fixed or rejected findings with reasons, validation performed, residual risks, dirty state, and parent/human decision needs.
- Post-merge validation baseline is explicit: focused Ticket/dossier tests, `cargo fmt --check`, `git diff --check`, `target/debug/yoi ticket doctor` where applicable, plus broader validation including `cargo check --workspace --all-targets` and `nix build .#yoi` when risk/touched files warrant it.
- Tests now assert the required dossier fields and validation baseline/broader-validation guidance.
- Merge authority remains explicit; conservative/missing authorization stops at the dossier and does not infer authority from public/default config.
- Branch-local reviewer verdict vs final main Ticket approval boundary remains preserved.
- Diff remains limited to `crates/client/src/ticket_role.rs`; no queue/worktree/coder/reviewer scheduler or TUI behavior was added.

Reviewer validation:
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `git merge-tree --write-tree develop HEAD`

Operational note from reviewer: main workspace dirty-state should be checked before merge; observed dirtiness was Ticket files/backend lock from orchestration records, not a branch implementation issue.

---

<!-- event: state_changed author: hare at: 2026-06-07T06:47:03Z from: ready to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T06:47:03Z status: closed -->

## Closed

Implemented, reviewed, merged, and validated.

Summary:
- Added Orchestrator merge-completion guidance for reviewed in-progress Tickets with merge-ready dossiers.
- Merge authority is explicit: dogfooding/workspace policy may authorize merge/cleanup/close; conservative/missing authorization stops at the dossier.
- Dossier requirements are explicit: Ticket id/slug, branch/worktree, commits, intent/invariant check, implementation summary, coder/reviewer Pods, blockers fixed or rejected findings with reasons, validation performed, residual risks, dirty state, and parent/human decision needs.
- Post-merge validation baseline is explicit: focused Ticket/dossier tests, `cargo fmt --check`, `git diff --check`, `target/debug/yoi ticket doctor` where applicable, and broader validation such as `cargo check --workspace --all-targets` / `nix build .#yoi` when risk/touched files warrant it.
- Branch-local reviewer verdicts remain dossier evidence before merge; final main Ticket review/approval/completion records are written during merge-completion after confirming the reviewed branch is the branch being merged.
- Guidance covers merge, post-merge validation, Ticket `done`/close handling, Pod scope reclaim, and worktree/branch cleanup.
- Queue routing and worktree/coder/reviewer routing were not reimplemented in this ticket.

Implementation:
- Child commits: `cb17728 orchestrator: add merge completion guidance`, `33abf3f fixup! orchestrator: add merge completion guidance`
- Merge commit: `merge: orchestrator merge completion`

Review:
- External reviewer `orchestrator-merge-reviewer-20260607` requested changes for explicit dossier/validation requirements.
- Fixup addressed blockers.
- Reviewer approved after fixup.

Validation after merge:
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`

---
