<!-- event: create author: LocalTicketBackend at: 2026-06-08T03:29:11Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: intake at: 2026-06-08T03:31:05Z -->

## Decision

Intake readiness classification: `implementation_ready` for routing, with `needs_preflight: true`.

Rationale: the Ticket already defines the product boundary between `worker.language` and durable Ticket record language, names the expected config shape (`[ticket].language` preferred), preserves absent-config behavior, and lists observable acceptance criteria. Preflight is still required before implementation because the work touches Ticket config, role launch prompts, typed tool/backend generated text, profile/worker language boundaries, and generated durable records.

Risk flags: `config`, `profile-boundary`, `workflow-prompts`, `tool-generated-records`, `backward-compatibility`, `localization-policy`.

Implementation latitude: implementers may choose the minimal practical set of generation paths for the first change as long as they document the policy, preserve existing behavior when `ticket.language` is absent, avoid rewriting existing Tickets, and escalate before changing public/authority semantics beyond Ticket record language selection.

---

<!-- event: intake_summary author: intake at: 2026-06-08T03:31:10Z -->

## Intake summary

Existing Ticket refined and marked ready for Orchestrator routing. Scope: add explicit Ticket record language configuration separate from Pod/worker response language; expose it to role launch prompts and typed Ticket-generated record paths where practical; preserve absent-config behavior and avoid translating existing records or protocol literals. Readiness: implementation_ready for routing, with preflight recommended/required because the work touches config, profile/worker language boundaries, workflow prompts, and generated durable Ticket records. Risk flags: config, profile-boundary, workflow-prompts, tool-generated-records, backward-compatibility, localization-policy.

---

<!-- event: state_changed author: intake at: 2026-06-08T03:31:10Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed; Ticket has enough requirements, acceptance criteria, invariants, validation expectations, and escalation conditions for Orchestrator routing. Do not start implementation automatically; the next human/panel action is ready -> queued.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T05:51:29Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T06:03:40Z -->

## Decision

Routing decision: preflight_needed

Reason:
- This Ticket touches the boundary between worker response language, durable Ticket record language, role launch prompts, Ticket tools/workflows, memory policy, and public/operator-facing project records.
- The desired behavior is clear at a high level, but the exact source of truth for Ticket record language and how it is propagated into role launch/tool/workflow contexts needs a binding design note before coder delegation.
- Without preflight, implementation could accidentally conflate user-facing conversation language with durable Ticket record language, or create hidden context-only language instructions that conflict with history/prompt-cache policy.

Evidence checked:
- Ticket body and thread, including latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; active worktrees for direct/delegation authority and panel close are separate.
- Code/prompt context: this crosses Ticket backend/tool wording, role launch prompt generation, workflow templates, profile/worker language handling, and memory language policy.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: canonical config key/source for Ticket record language, fallback/default behavior, relationship to `[worker].language` and `[memory].language`, which Ticket tools/workflows/role launch prompts must use record language, whether existing Tickets are left as-is, how mixed-language user input is handled, tests/fixtures, and explicit non-use of hidden context-only language injection.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/separate-ticket-record-language-from-worker-language`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Ticket record language requires a migration of existing Ticket content.
- The design would make worker response language override durable Ticket record language.
- Language guidance would need to be injected into LLM context without being represented in committed history/tool/workflow prompts.
- The implementation would blur Ticket record authority with memory-language generation rules.

---

<!-- event: decision author: orchestrator at: 2026-06-08T07:41:03Z -->

## Decision

Routing decision: implementation_ready

Correction:
- The previous `preflight_needed` decision was too conservative for the current Ticket state. The Ticket body and Intake summary already fix the product boundary: `worker.language` controls conversational prose, while `ticket.language` controls durable Ticket records and generated Ticket event/resolution/scaffold text.
- Remaining uncertainty is bounded implementation policy and test coverage, not a human-blocking design question.

Reason:
- The canonical config shape is already selected by the Ticket as `[ticket].language`, with explicit instruction not to overload `[worker].language` or Profile settings.
- Backward compatibility is clear: absence of `ticket.language` preserves current/default behavior.
- The non-migration rule is clear: existing Tickets are not bulk-translated or rewritten.
- The language boundary is explicit enough for coder/reviewer: do not translate protocol literals, file paths, commands, logs, identifiers, or quoted external text merely because Ticket language is set.
- Role-launch prompt exposure and typed generated-record coverage can be implemented incrementally where practical, as long as the policy is documented and tests cover representative paths.

Evidence checked:
- Ticket body and thread, including Intake readiness and prior routing note.
- Workspace state: no matching branch/worktree exists; active `allow-spawnpod-child-workspace-cwd` and `shutdown-intake-pod-after-ready-idle` worktrees are separate and touch different code paths.
- Code map search for Ticket config/scaffold, worker/memory language handling, Ticket role launch prompt/context, typed Ticket generated events/resolutions, and Panel close deterministic resolution paths.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Add explicit Ticket record language configuration and propagate it to durable Ticket record generation and role guidance without changing normal worker response language.

Binding decisions / invariants:
- `worker.language` remains conversation/prose response language for Pods.
- `memory.language` remains memory/Knowledge generation language.
- New `ticket.language` is durable Ticket record language for Ticket item/thread/resolution generated text and role guidance where Ticket records are written.
- Preferred config shape is `[ticket].language` in `.yoi/ticket.config.toml`.
- Absence of `ticket.language` preserves current/default behavior.
- Existing Tickets are not bulk-translated or rewritten.
- Do not translate protocol literals, file paths, commands, logs, identifiers, or quoted external text merely because Ticket language is configured.
- Do not implement hidden context-only language injection. Language guidance must come from committed config/tool/workflow/launch-prompt paths.
- Do not make worker response language override durable Ticket record language.

Requirements / acceptance criteria:
- Extend Ticket config parser/scaffold to support optional `[ticket].language`.
- Update this project's `.yoi/ticket.config.toml` to set the desired Ticket record language.
- Expose Ticket record language to Ticket role launch prompt/context so Intake/Orchestrator/Coder/Reviewer know which language to use for Ticket comments/reviews/decisions.
- Make typed Ticket-generated text consult configured Ticket record language where practical, at minimum for scaffold/default generated bodies or prominent generated event/resolution paths selected by coder.
- Preserve current behavior when `ticket.language` is absent.
- Document/encode the policy clearly enough that reviewer can distinguish implemented paths from future localization work.
- Add tests for config parsing/scaffold/default behavior and at least one role prompt or generated record path.

Implementation latitude:
- Coder may choose minimal practical first coverage for generated text paths, but must report which generated paths consult `ticket.language` and which remain future work.
- Coder may use simple language labels/instructions rather than full translation infrastructure.
- Coder may leave existing Ticket bodies/threads/resolutions untouched.
- Coder may update maintained workflow/prompt guidance if it directly tells agents what language to use for Ticket records.

Escalate if:
- Implementing `ticket.language` requires migrating existing Tickets.
- Typed Ticket backend localization would require broad translation catalogs rather than a minimal policy/instruction pass-through.
- Role launch prompt language guidance cannot be passed through committed prompt/history/tool paths.
- Any path would conflate Ticket record language with memory extraction language or normal worker prose language.

Validation:
- Ticket config parser/scaffold tests.
- Role launch prompt/context tests verifying Ticket record language is included independently from worker language.
- Focused generated-record test for at least one configured-language path.
- `cargo test -p ticket ...` / `cargo test -p client ...` / `cargo test -p yoi ...` focused equivalents selected by coder.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because config/scaffold/runtime prompt behavior may change, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/ticket/src/config.rs` and Ticket config scaffold helpers.
- `crates/yoi/src/ticket_cli.rs`: `yoi ticket init` scaffold tests and CLI examples.
- `crates/client/src/ticket_role.rs`: `TicketRoleLaunchContext`, role prompt rendering, launch prompt construction.
- `crates/tui/src/multi_pod.rs` and `workspace_panel.rs`: role launch/context construction and Panel generated resolution paths.
- `crates/ticket/src/lib.rs`: typed Ticket generated events, default bodies, close behavior.
- `.yoi/ticket.config.toml`: project-local desired Ticket record language.
- `resources/prompts` / maintained workflow files only if they contain active role guidance that should mention Ticket record language.

Critical risks / reviewer focus:
- `ticket.language` must remain independent from `worker.language`.
- Existing absent-config behavior must not regress.
- Role prompt guidance must be durable/committed, not hidden context injection.
- Existing records must not be bulk rewritten.
- Generated text coverage must be explicit; partial coverage is acceptable only if policy/future-work boundary is clear.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T07:41:11Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after user confirmation and routing IntentPacket were recorded. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T07:41:39Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `separate-ticket-record-language-from-worker-language`
- Worktree: `.worktree/separate-ticket-record-language-from-worker-language`
- Base: `17ac5b0 ticket: route ticket record language implementation`
- Scope: add optional Ticket record language config and propagate it to role guidance/generated Ticket-record paths where practical.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/separate-ticket-record-language-from-worker-language`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Concurrency note:
- `allow-spawnpod-child-workspace-cwd` and `shutdown-intake-pod-after-ready-idle` are active in separate worktrees. This language-policy implementation must remain isolated and should not depend on those unmerged branches.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T08:04:24Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-ticket-record-language`
- Commit: `fb261bb feat: add ticket record language config`
- Worktree status: clean after commit.
- Diff against `develop`: 9 files changed, 310 insertions, 28 deletions.

Files touched by coder:
- `.yoi/ticket.config.toml`
- `crates/ticket/src/config.rs`
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `crates/client/src/ticket_role.rs`
- `crates/pod/src/feature/builtin/ticket.rs`
- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/yoi/src/ticket_cli.rs`

Coder reported implementation:
- Added optional `[ticket].language` parsing with blank-string rejection and a commented scaffold example in `yoi ticket init` output.
- Set this repository's `.yoi/ticket.config.toml` to `[ticket] language = "Japanese"`.
- Added `ticket_record_language` to `TicketRoleLaunchContext` and role launch prompt guidance, independent from worker/memory language.
- Added `LocalTicketBackend::with_record_language()` / `record_language()` and propagates config language through CLI, built-in Ticket feature, and Panel backend construction.
- Added minimal generated-text coverage for configured Japanese: create default item body, initial create thread event, queue/status/close generated events, `TicketIntakeReady` default body, and workspace Panel auto-close resolution.
- No existing Tickets were bulk-translated or rewritten.
- Arbitrary language names flow to role prompt/context; typed backend generated strings use explicit Japanese handling and otherwise fall back to default English rather than pretending to be a full localization catalog.

Coder reported validation:
- `cargo test -p ticket config::tests --lib`
- `cargo test -p ticket create_uses_configured_japanese_record_language_for_generated_defaults --lib`
- `cargo test -p client configured_ticket_record_language_is_included_in_role_prompt --lib`
- `cargo test -p yoi ticket_cli_init_writes_explicit_ticket_config_scaffold`
- `cargo check -p pod -p tui`
- `cargo test -p ticket --lib`
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-ticket-record-language`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by current launch cwd behavior.
- Reviewer was instructed to judge against the recorded Ticket requirements and binding decisions.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred for this Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T08:18:46Z -->

## Implementation report

Merge-ready dossier: Ticket record language policy

Ticket id/slug:
- `20260608-032911-separate-ticket-record-language-from-worker-language` / `separate-ticket-record-language-from-worker-language`

Branch/worktree:
- Branch: `separate-ticket-record-language-from-worker-language`
- Worktree: `.worktree/separate-ticket-record-language-from-worker-language`
- Current branch commit:
  - `fb261bb feat: add ticket record language config`

Intent / invariant check:
- `worker.language` remains conversational response language.
- `memory.language` remains memory/Knowledge generation language.
- New `ticket.language` config controls durable Ticket record language and role guidance where Ticket records are written.
- Existing Tickets were not bulk-translated or rewritten by the implementation commit.
- Role prompt guidance is committed launch input, not hidden context-only injection.
- Non-translateable literals such as paths, commands, logs, identifiers, and quoted text remain outside language conversion policy.

Implementation summary:
- Added optional `[ticket].language` config parsing and blank-string validation.
- Added scaffold/example support for `yoi ticket init`.
- Set this repository's `.yoi/ticket.config.toml` to `[ticket] language = "Japanese"`.
- Added Ticket record language to `TicketRoleLaunchContext` and role launch prompt guidance independent from worker/memory language.
- Added `LocalTicketBackend::with_record_language()` / `record_language()` and propagated configured language through CLI, built-in Ticket feature, and Panel backend construction.
- Added minimal generated-text coverage for Japanese, including create default body/thread, prominent state/close/intake-ready generated events, and Panel auto-close resolution.
- Preserved default behavior when config is absent and falls back to English/default generated text for unsupported backend generated languages.

Files touched:
- `.yoi/ticket.config.toml`
- `crates/ticket/src/config.rs`
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `crates/client/src/ticket_role.rs`
- `crates/pod/src/feature/builtin/ticket.rs`
- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/yoi/src/ticket_cli.rs`

Coder / reviewer Pods:
- Coder: `coder-ticket-record-language`
- Reviewer: `reviewer-ticket-record-language`

Review evidence:
- Reviewer verdict: `approve`.
- Reviewer confirmed config parsing/scaffold/default/blank rejection, role prompt propagation independent from worker/memory language, durable prompt input path rather than hidden context injection, config propagation through CLI/feature/Panel backends, and no implementation-commit bulk rewrite of existing Tickets.

Validation performed by coder and/or reviewer:
- `cargo test -p ticket config::tests --lib`
- `cargo test -p ticket create_uses_configured_japanese_record_language_for_generated_defaults --lib`
- `cargo test -p client configured_ticket_record_language_is_included_in_role_prompt --lib`
- `cargo test -p yoi ticket_cli_init_writes_explicit_ticket_config_scaffold`
- `cargo test -p client ticket_role --lib`
- `cargo test -p ticket --lib`
- `cargo check -p pod -p tui`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Blockers fixed or rejected findings:
- No reviewer blockers.

Residual risks:
- Japanese generated-text coverage is intentionally minimal, not full localization. Generic thread headings, some Panel/Defer decision bodies, and tool success output may remain English unless separately localized later.
- Japanese handling helpers are duplicated between Ticket crate and TUI for now; centralization may be useful if supported languages or generated paths expand.
- Branch was reviewed before later main merges that touched `crates/client/src/ticket_role.rs`; merge-completion must preserve both this Ticket's language guidance and later cwd/intake role guidance.

Dirty state:
- Child worktree is clean at `fb261bb`.
- Main workspace has unrelated Ticket-record edits for queued/intake work; they are outside this branch's core implementation paths and are understood.

Parent/human decision needs:
- User has authorized merge-completion and cleanup after approved work. Proceeding to merge-completion unless merge conflict or post-merge validation fails.

---
