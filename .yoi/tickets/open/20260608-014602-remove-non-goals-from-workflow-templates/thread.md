<!-- event: create author: LocalTicketBackend at: 2026-06-08T01:46:02Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: intake at: 2026-06-08T01:46:53Z -->

## Decision

Intake classification: this existing Ticket is sufficiently specified for Orchestrator routing.

- readiness: implementation_ready
- needs_preflight: false
- risk_flags: [workflow-prompts]

Rationale: the requested change is constrained to workflow/prompt wording and intent packet examples; it preserves escalation conditions, validation, concrete exclusions as binding decisions, and the Ticket state machine. No authority boundary, persistence, public API, security, or storage migration decision is left for a coder to make silently.

---

<!-- event: intake_summary author: intake at: 2026-06-08T01:46:58Z -->

## Intake summary

Existing Ticket refined and classified as implementation_ready. Scope is removal of generic Non-goals/Non-goals constraints sections and broad non-goal prompts from workflow templates and intent packet examples, replacing boundary expression with Binding decisions / invariants plus Implementation latitude. Preserve escalation conditions, validation, concrete exclusions as binding decisions, and the Ticket state machine. needs_preflight=false; risk_flags=[workflow-prompts].

---

<!-- event: state_changed author: intake at: 2026-06-08T01:46:58Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake complete: requirements, acceptance criteria, implementation boundaries, validation, and routing signals are sufficient for Orchestrator review/queueing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T01:48:07Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T01:50:29Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Intake classified this Ticket as `implementation_ready`, `needs_preflight=false`; the requested change is scoped to workflow/prompt wording and intent packet examples.
- Requirements and acceptance criteria are observable: remove generic `Non-goals` / `Non-goals / constraints` workflow sections and broad `やらないことは何か。` prompts, while preserving concrete boundaries as `Binding decisions / invariants`, `Implementation latitude`, escalation conditions, and validation.
- The code/workflow search shows the active maintained workflow-template hits are concentrated in `.yoi/workflow/{ticket-intake-workflow.md,ticket-orchestrator-routing.md,ticket-preflight-workflow.md,multi-agent-workflow.md}`. Historic Ticket artifacts contain old wording and should not be rewritten.
- Remaining uncertainty is bounded text/test cleanup, not a design-boundary decision.

Evidence checked:
- Ticket body and thread, including intake decision and queued event.
- Workspace state: current main workspace has only this new Ticket record uncommitted; runtime workspace identity bundle is in a separate worktree/re-review and should remain isolated.
- Code/workflow search for `Non-goals`, `Non-goals / constraints`, `Non-goals / invariants`, and `やらないことは何か。`.
- Ticket doctor: 0 errors.

IntentPacket:

Intent:
- Remove generic `Non-goals` workflow-template language and make workflow examples express boundaries through concrete invariants and implementation latitude instead.

Binding decisions / invariants:
- Do not rewrite historic closed/open Ticket threads or artifacts just because they contain old `Non-goals` text.
- Preserve the ability to state a concrete exclusion when it is genuinely a binding decision, invariant, authority boundary, or escalation condition.
- Preserve escalation conditions, validation sections, reviewer guidance, and the Ticket workflow/state model.
- Maintained workflow templates should avoid broad optional exclusion buckets that invite generic unrelated lists.

Requirements / acceptance criteria:
- Remove `Non-goals` / `Non-goals / constraints` sections from maintained workflow templates and IntentPacket examples.
- Remove Intake prompts such as `やらないことは何か。` that ask agents to enumerate generic non-goals.
- Replace relevant boundary examples with `Binding decisions / invariants` and `Implementation latitude` language.
- Keep reviewer guidance focused on recorded intent, binding decisions/invariants, implementation latitude, acceptance criteria, and explicit escalation conditions.
- Update prompt/workflow tests or role prompt guidance if any maintained test expects old section names.

Implementation latitude:
- Coder may choose exact wording, section order, and whether a specific concrete exclusion is better expressed as an invariant or escalation condition.
- Coder may update adjacent maintained workflow-template wording where needed for consistency, but should avoid broad documentation churn.

Escalate if:
- A workflow genuinely requires a separate broad exclusion bucket after rewriting.
- Removing old wording would obscure a real authority boundary rather than clarify it.
- Required updates extend beyond maintained workflow/prompt templates into historic Ticket artifacts or unrelated docs.

Validation:
- Search maintained workflows/prompts for old generic section names and broad `やらないこと` prompt.
- Run focused prompt/workflow tests if present.
- `target/debug/yoi ticket doctor` or `cargo run -q -p yoi -- ticket doctor`.
- `cargo fmt --check` only if Rust/test code changes.
- `git diff --check`.
- `nix build .#yoi` is required if runtime resources/prompts packaged into the product are changed; if only `.yoi/workflow` project records change, explain why Nix build was omitted.

Current code map:
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`
- Search broader maintained prompt/test paths for old labels before reporting.

Critical risks / reviewer focus:
- Do not simply rename `Non-goals` to another generic exclusion bucket.
- Do not remove concrete authority boundaries or escalation conditions.
- Do not edit historical Ticket records as part of cleanup.
- Ensure workflow examples still give coder/reviewer enough boundary information.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T01:50:34Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and maintained workflow-template hits. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T01:50:56Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `remove-non-goals-workflow-templates`
- Worktree: `.worktree/remove-non-goals-workflow-templates`
- Base: `b45d8f1 ticket: route workflow template non-goals cleanup`
- Initial maintained workflow hits:
  - `.yoi/workflow/ticket-intake-workflow.md`
  - `.yoi/workflow/ticket-orchestrator-routing.md`
  - `.yoi/workflow/ticket-preflight-workflow.md`
  - `.yoi/workflow/multi-agent-workflow.md`

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/remove-non-goals-workflow-templates`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not rewrite historic Ticket artifacts.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Stop condition:
- This Orchestrator will stop at a merge-ready dossier; no merge, close, final approval, or cleanup in this phase.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T02:05:05Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-remove-non-goals-workflow`
- Commit: `783cd42 workflow: replace non-goals template language`
- Worktree status: clean after commit.
- Diff against `develop`: 6 files changed, 38 insertions, 43 deletions.

Files touched by coder:
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`
- `crates/client/src/ticket_role.rs`
- `docs/development/work-items.md`

Coder reported validation:
- `cargo fmt --check`
- `cargo test -p client ticket_role`
- maintained-hit search excluding historic Tickets and generated/target paths
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-remove-non-goals-workflow`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by launch cwd validation.
- Reviewer was instructed to judge against the recorded Ticket requirements and binding decisions.

Pending:
- Await reviewer verdict before merge-ready dossier.
- No merge, close, final approval, or cleanup has occurred for this Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T02:21:20Z -->

## Implementation report

Merge-ready dossier: workflow template non-goals cleanup

Ticket id/slug:
- `20260608-014602-remove-non-goals-from-workflow-templates` / `remove-non-goals-from-workflow-templates`

Branch/worktree:
- Branch: `remove-non-goals-workflow-templates`
- Worktree: `.worktree/remove-non-goals-workflow-templates`
- Current branch commit:
  - `783cd42 workflow: replace non-goals template language`

Intent / invariant check:
- Maintained workflow templates and role guidance no longer use generic `Non-goals` / `Non-goals / constraints` buckets or the broad `やらないことは何か。` prompt.
- Concrete exclusions remain expressible as binding decisions, invariants, authority boundaries, implementation latitude, and escalation conditions.
- Historical Ticket records/artifacts were not rewritten.
- Ticket workflow/state model, validation guidance, reviewer basis, and escalation guidance were preserved.

Implementation summary:
- Rewrote maintained workflow templates to replace generic non-goals language with `Binding decisions / invariants`, `Implementation latitude`, and concrete boundary wording.
- Updated Ticket role prompt guidance in `crates/client/src/ticket_role.rs` so reviewer/coder basis no longer includes broad non-goals buckets.
- Updated development work-item documentation to match the maintained workflow language.

Files touched:
- `.yoi/workflow/ticket-intake-workflow.md`
- `.yoi/workflow/ticket-orchestrator-routing.md`
- `.yoi/workflow/ticket-preflight-workflow.md`
- `.yoi/workflow/multi-agent-workflow.md`
- `crates/client/src/ticket_role.rs`
- `docs/development/work-items.md`

Coder / reviewer Pods:
- Coder: `coder-remove-non-goals-workflow`
- Reviewer: `reviewer-remove-non-goals-workflow`

Review evidence:
- Reviewer verdict: `approve`.
- Reviewer confirmed maintained templates/examples remove old `Non-goals` wording and replace it with binding decisions/invariants, implementation latitude, and escalation-focused language.
- Reviewer confirmed Intake no longer asks the broad `やらないことは何か。` prompt.
- Reviewer confirmed no historic Ticket artifacts were touched.

Validation performed by coder and/or reviewer:
- `cargo fmt --check`
- `cargo test -p client ticket_role`
- maintained old-term search excluding historic Tickets and generated paths
- `git diff --check develop...HEAD`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi --no-link`

Blockers fixed or rejected findings:
- No reviewer blockers.

Residual risks:
- Some Japanese role/scope sections still say a workflow does not do or handle specific things. Reviewer judged these as concrete role/authority/scope boundaries, not generic optional non-goals buckets.

Dirty state:
- Child worktree is clean at `783cd42`.
- Main workspace has unrelated Ticket-record changes for other active/new Tickets; they are outside this branch's touched paths and are understood.

Parent/human decision needs:
- User has authorized merge-completion and cleanup after approved work. Proceeding to merge-completion unless validation fails.

---
