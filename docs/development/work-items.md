# Tickets and development workflow

Yoi project work is tracked through Tickets. For normal use, interact with Tickets through `yoi panel`, Ticket tools, the `yoi ticket ...` CLI, and typed role surfaces. Git history plus Ticket files remain the authoritative state-transition record behind those interfaces.

The current local backend stores each Ticket in the flat `.yoi/tickets/<ticket-id>/` layout. The directory name is the canonical opaque Ticket id: a fixed-width Crockford base32 Unix epoch millisecond timestamp. Slugs and frontmatter `id`/`slug` fields are not current-state authority. That storage detail matters for maintainers and backend compatibility, but it is not the primary user-facing workflow.

Do not treat ad-hoc chat summaries, memory records, or Worker notifications as the final source of project state. Notifications are hints to inspect concrete state, not proof of completion.

## Concepts

- `Ticket`: durable project/orchestration record. It contains requirements, decisions, plans, implementation reports, reviews, artifacts, and resolution history.
- `Objective`: first-class medium-term goal record. It stores goal, motivation/background, strategy/design direction, success criteria/exit conditions, decision context, current Objective lifecycle, and canonical Ticket links under `.yoi/objectives/<objective-id>/item.md`. Objective context is judgment/background context; it is not implementation authority and does not replace reading each Ticket body/thread/artifacts.
- `Task`: session-local progress tracking inside a Worker. It is not the project record.
- `Assignment`: a concrete delegation from an Orchestrator to a coder/reviewer Worker or task-specific helper Worker.
- `IntentPacket`: the short implementation/review contract derived from a Ticket and handed to an Assignment.
- `LocalTicketBackend`: the current `.yoi/tickets/` markdown/thread/artifacts storage backend.
- `Ticket relation`: durable project-level Ticket-to-Ticket metadata stored as forward canonical-id relations (`depends_on`, `blocks`, `related`, `supersedes`, `duplicate_of`). Inverse views such as `blocked_by` are derived, not stored.

A Ticket may represent a feature, bug, cleanup, design decision, investigation, workflow change, release task, or orchestration task. The common requirement is that the Ticket is a concrete work item that can be implemented, reviewed, validated, and closed on its own terms.

## User-facing entry points

Use the highest-level interface that matches the work:

- Use `yoi panel` for the Ticket/Intake/Orchestrator workspace Dashboard and role-launch actions.
- Use `yoi objective ...` for lightweight medium-term Objective records and their non-blocking canonical Ticket links.
- Inside Workers, use typed Ticket tools for Ticket records and typed Merge Request tools for immutable implementation/review/completion evidence.
- For multi-step work, follow the typed Ticket role surfaces and recorded Ticket lifecycle gates.

Maintainers can inspect the local `.yoi/tickets/` files directly when debugging storage, but normal user instructions should go through `yoi panel`, Ticket tools, or `yoi ticket ...`.

## Ticket tools inside Workers

Workers with the Ticket built-in feature can use typed Ticket tools:

- `TicketCreate`
- `TicketList` — lightweight bounded overview for selecting ids; it returns short summaries only and must not be used as body/thread/artifact authority.
- `TicketShow` — detailed authority for a single Ticket, including body/thread/artifact metadata/resolution context subject to its own bounds.
- `TicketComment`
- `MergeRequestShow`, `MergeRequestOpen`, `MergeRequestAddRevision`, `MergeRequestComplete`
- `MergeRequestReviewSubmit` — available only inside the attested direct-child Reviewer attempt; attempt/revision capability material is not model input.
- `TicketClose`
- `TicketRelationRecord`
- `TicketRelationQuery`
- `TicketDoctor`

These tools operate through the typed Ticket backend. They are not arbitrary filesystem write permission to `.yoi/tickets/`.

Relation tools are for non-hierarchical project metadata only. Use canonical opaque Ticket ids, store forward relations only, and keep runtime execution planning (capacity, ordering decisions, do-not-parallelize notes, Worker/session/worktree ownership) in OrchestrationPlan or session-local records instead of relation metadata. Unresolved `depends_on` and incoming unresolved `blocks` are queue/acceptance blockers; `related` is not blocking, and `supersedes` / `duplicate_of` are diagnostics rather than automatic lifecycle transitions.

Use them when a Worker needs to materialize or update project records:

- Intake creates a new Ticket after user agreement.
- Orchestrator records routing decisions and intent packets.
- Reviewer commits an approve/request-changes result against one immutable Merge Request revision.
- Maintainer closes a Ticket with a resolution when merge/validation/cleanup evidence is complete.

Do not bypass Ticket lifecycle gates just because Ticket tools are available. Ticket mutation is a project-record operation and should remain auditable.

## Objective records

Objectives are lightweight medium-term project records, not Tickets, Ticket relations, OrchestrationPlan execution records, or Worker/session claims. Use them when a goal spans several concrete Tickets and the durable motivation, design direction, success criteria, or decision context would otherwise be repeated or lost.

The local Objective surface stores records under:

```text
.yoi/objectives/<objective-id>
  item.md
```

`<objective-id>` is the canonical opaque path-derived id: a fixed-width Crockford base32 Unix epoch millisecond timestamp. Do not treat Objective titles or slug words as link authority.

`item.md` uses YAML frontmatter plus Markdown body:

```yaml
---
title: "Improve orchestration evidence"
state: "active"        # active|paused|done|archived
created_at: "2026-06-09T00:00:00Z"
updated_at: "2026-06-09T00:00:00Z"
linked_tickets: ["00001KTKMS0VG"]
---
```

The Markdown body should include these sections:

- `## Goal`
- `## Motivation / background`
- `## Strategy / design direction`
- `## Success criteria / exit conditions`
- `## Decision context`

Linked Tickets must be canonical opaque Ticket ids that exist in the configured Ticket backend root. Objective-to-Ticket links are context links only: they are not dependency, blocking, ordering, ownership, or scheduling relations. Use typed Ticket relations for Ticket-to-Ticket dependency/blocking/related metadata, OrchestrationPlan records for routing/execution plans, and Worker/session claims for runtime ownership hints.

Objective lifecycle is only Objective lifecycle. `active`, `paused`, `done`, and `archived` do not drive Ticket `state`, do not authorize implementation, and do not close linked Tickets. A role reading Objective context must still inspect each Ticket body, thread, artifacts, explicit Ticket relations, and OrchestrationPlan records before acting.

The maintainer CLI is:

```sh
yoi objective create --title "..." [--ticket <ticket-id> ...]
yoi objective list [--state active|paused|done|archived|all]
yoi objective show <objective-id>
yoi objective doctor
```

The first version intentionally does not implement roadmap scheduling, milestones, OKRs, graph solving, Objective-mandatory Ticket creation, Objective thread/artifact history, or broad panel UX. Future UX can surface Objective context around Tickets as long as it remains background context and never substitutes for reading the Ticket.

## Ticket configuration

Workspace Ticket policy is configured by the tracked workspace settings file `.yoi/workspace.toml` under the `[ticket]` table. The old `.yoi/ticket.config.toml` file is obsolete: current code only reads it as a narrow read-only migration fallback when `.yoi/workspace.toml` has no `[ticket]` table. Workspace settings take precedence as soon as `[ticket]` exists.

MVP shape:

```toml
[ticket]
language = "Japanese"

[ticket.backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"

[ticket.roles.intake]
profile = "project:intake"
launch_prompt = "ticket.intake.launch"

[ticket.roles.orchestrator]
profile = "project:orchestrator"
launch_prompt = "ticket.orchestrator.launch"

[ticket.roles.coder]
profile = "project:coder"
launch_prompt = "ticket.coder.launch"

[ticket.roles.reviewer]
profile = "project:reviewer"
launch_prompt = "ticket.reviewer.launch"
```

Fixed roles are:

- `intake`
- `orchestrator`
- `coder`
- `reviewer`

This is not an arbitrary role registry. The fixed roles are the roles required by Ticket orchestration.
Stale `[ticket.roles.investigator]` config is rejected as an unsupported fixed role; remove it and,
when a spike is useful, let the Orchestrator create an ordinary task-specific read-only helper Worker.

`profile` selects the Worker runtime Profile for that role. The selected Profile owns durable role/system behavior. Workspace Ticket settings do not have a role-level `system_instruction` field.

`launch_prompt` is a per-action first-run prompt reference for future prompt resolution. Current launcher behavior exposes the ref but does not treat it as system instruction.

Role launch prompts are plain history input. State and phase-specific prompt injection are future work; any dynamic prompt content must be committed as history before it affects model context.

`provider = "builtin:yoi_local"` selects Yoi's built-in local Ticket backend. `root = ".yoi/tickets"` is the canonical local storage root for this repository. Legacy `kind = "local"` is accepted only as a short transitional alias; new configs should use `provider`.

If `.yoi/workspace.toml` has no `[ticket]` table and no legacy fallback file exists, defaults are:

- backend provider: `builtin:yoi_local`
- backend root: `<workspace>/.yoi/tickets`
- all role profiles: `inherit`
- no launch prompt refs

Important: top-level Ticket role launches cannot execute `profile = "inherit"` because top-level launch has no parent Profile to inherit from. Configure concrete role profiles in `.yoi/workspace.toml` under `[ticket.roles.*]` before using `yoi panel` role-launch actions.

## Ticket lifecycle

Ticket-driven development normally moves through these gates:

1. Intake
2. Orchestrator routing
3. Planning/requirements sync or spike when needed
4. Implementation assignment
5. Review
6. Merge / validation / cleanup
7. Close

Each gate records its decision or evidence in the Ticket thread or artifacts.

### 1. Intake

Use the Intake role launch prompt when a user request is broad, ambiguous, or not yet a Ticket.

Intake should:

- clarify user intent;
- check duplicate/related Tickets;
- draft background, requirements, acceptance criteria, binding decisions/invariants, implementation latitude, readiness, risk flags, and validation;
- create or update the Ticket only after user agreement.

Intake should not schedule implementation, spawn coder/reviewer Workers, create worktrees, merge, or close Tickets.

### 2. Orchestrator routing

Use the Orchestrator role launch prompt to classify the next action for an existing Ticket.

Routing classifications include:

- `requirements_sync_needed`
- `return_to_planning`
- `spike_needed`
- `implementation_ready`
- `review_needed`
- `blocked_by_dependency_or_missing_authority`
- `close_ready`
- `closed_or_noop`

Routing decisions should be recorded with `TicketComment` using `plan` or `decision` role. The decision should state the classification, evidence checked, reason, next action, and escalation conditions. For `return_to_planning`, the record must also state the concrete missing decision/information, context checked, why implementation latitude is insufficient, and the next planning question/action.

### 3. Planning/requirements sync

Use planning/requirements sync only as a bounded Ticket refinement step. Return `ready` or `queued` Tickets to `planning` only when the Orchestrator can name a concrete missing decision or information item after bounded project-context checks; risk flags and risky domains are context-lookup and reviewer-focus signals, not automatic stop gates.

Planning sync should resolve or record:

- requirements and acceptance criteria;
- current code map;
- binding decisions/invariants and implementation latitude;
- critical risks and failure modes;
- implementation-ready vs requirements-sync/spike/blocked classification.

Do not send Tickets with unresolved concrete missing decisions/information directly to coder Workers. If no concrete missing item remains after bounded checks, risky-but-specified Tickets should proceed with an IntentPacket plus escalation conditions and reviewer focus.

### 4. Implementation assignment

Use the Coder and Reviewer role launch prompts for implementation-ready Tickets.

The Orchestrator should prepare an `IntentPacket` with:

- intent;
- requirements;
- binding decisions/invariants;
- implementation latitude;
- escalation conditions;
- validation;
- current code map;
- critical risks.

Implementation normally happens in a child git worktree created by the Orchestrator, not by the coder Worker. The coder Worker receives narrow write scope to the worktree and must report changed files, implementation summary, validation, unresolved risks, and review readiness.

### 5. Review

The assigned Coder launches the Reviewer as an actual direct-child `builtin:reviewer` SubWorker with read-only scope and a structured handoff bound to the current immutable Merge Request revision. Server authority revalidates the parent assignment, Runtime-owned child session, effective profile, one-shot review attempt, and revision; prose output is not approval.

The Reviewer records the structured result with `MergeRequestReviewSubmit`. Request changes requires a new immutable revision and a fresh child attempt. `MergeRequestComplete` performs guarded Ticket completion with operation-id dedupe/CAS semantics; Flow transitions are not completion authority.

Blockers must be fixed or explicitly escalated before merge-ready submission.

### 6. Merge and close

Unless explicitly authorized otherwise, final merge, cleanup, design-boundary decisions, and Ticket closure remain Orchestrator/human responsibilities.

Before closing, verify concrete evidence:

- SubWorker committed session via worker-observation tools;
- worktree state and diff;
- validation command output;
- review result;
- Ticket requirements and acceptance criteria;
- merge/cleanup state in the main workspace.

Close with a resolution that summarizes what changed, key commits, validation, review state, and remaining follow-ups.

## Workspace Dashboard Ticket role actions

`yoi panel` is the active Ticket/Intake/Orchestrator Dashboard. It owns fixed Ticket role-launch actions and uses the shared client Ticket role launcher. The single-Worker Console no longer supports `:ticket ...` commands; typing them in command mode is treated like any other unknown command.

Role actions map to the same fixed roles configured in `.yoi/workspace.toml` under `[ticket.roles]`:

- intake launches the intake role without an existing Ticket and requires freeform context.
- route launches the orchestrator role for an existing Ticket.
- implement launches the coder role for an implementation assignment.
- review launches the reviewer role for review.

All actions are explicit and user-triggered. They are not a scheduler, queue, spawned-Worker Dashboard, or automatic maintainer loop.

### Dashboard execution path

The role-launch path is:

```text
User triggers a Ticket action in yoi panel
  -> Dashboard builds a TicketRoleLaunchContext
  -> client Ticket role launcher reads .yoi/workspace.toml [ticket] settings
  -> launcher selects the role Profile
  -> launcher spawns the role Worker
  -> launcher sends Method::Run with Text segments
  -> launcher waits for run-acceptance evidence
  -> Dashboard reports success/failure
```

The launched Worker receives dynamic Ticket/action context as its first committed run input. The Dashboard does not inject hidden context, does not write Ticket files directly, and does not construct prompt segments by hand.

The first run input contains:

- the selected fixed role;
- Ticket id when the action targets an existing Ticket;
- freeform user instruction/context from the action;
- configured `launch_prompt` reference if present, as an unresolved reference for future prompt resolution.

The selected Profile supplies durable system/role behavior. Workspace Ticket settings do not override system instruction.

### Dashboard setup

Because top-level role launches cannot inherit a parent Profile, configure concrete role profiles before using Dashboard role actions:

```toml
# .yoi/workspace.toml

[ticket.backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"

[ticket.roles.intake]
profile = "project:intake"

[ticket.roles.orchestrator]
profile = "project:orchestrator"

[ticket.roles.coder]
profile = "project:coder"

[ticket.roles.reviewer]
profile = "project:reviewer"
```

If a role still uses `profile = "inherit"`, the Dashboard fails closed with a diagnostic explaining that a concrete profile is required.

### Dashboard troubleshooting

- `profile = "inherit"`: configure a concrete role Profile in `.yoi/workspace.toml` under `[ticket.roles.<role>]`.
- malformed workspace Ticket settings: fix the `[ticket]` table in `.yoi/workspace.toml` and retry.
- missing Ticket id for route, implement, or review actions: provide the target Ticket.
- launch success but no visible completion: attach to or inspect the launched Worker; completion notifications are hints, not authority.

## Granularity

One Ticket should describe a complete change that can be explained as a feature, behavior, design decision, investigation result, or maintenance outcome when closed. It should be concrete enough to implement, review, validate, and close without relying on another open Ticket as its progress container.

Avoid Tickets that only mirror an implementation step unless that step is independently reviewable and useful. Phase/step lists inside a Ticket are execution order, not a separate dependency system.

Do not create new umbrella Tickets for broad multi-Ticket efforts. When a request is too broad for one concrete work item:

- create concrete implementable Tickets for the slices;
- record the split decision in the relevant Ticket thread, Objective context, or both;
- use Objectives for medium-term goal, motivation, strategy, and success-criteria context when that context would outlive one concrete Ticket;
- once typed Ticket relations exist, use them only for non-hierarchical dependency, related, blocking, superseded-by, duplicate, or replacement metadata;
- do not replace umbrellas with parent/child, sub-ticket, umbrella, part-of, contains, or other hierarchy/container relations;
- do not keep a separate umbrella Ticket open merely as a progress container.

This policy does not forbid an initial concrete planning, design, or investigation Ticket when the user asks for one. The deprecated pattern is a long-lived umbrella/progress-container Ticket whose main purpose is to keep a broad effort open while other concrete Tickets carry the actual work.

Existing umbrella Tickets may be retired without rewriting history. Once concrete follow-up Tickets and any needed Objective context exist, close the umbrella as superseded/decomposed. The close resolution should state that the container role is retired, not that every related future concern is complete, and should list completed concrete Tickets plus remaining follow-up Tickets/Objectives.

## Ticket contents

A useful Ticket states:

- background and motivation;
- requirements;
- acceptance criteria;
- relevant binding decisions/invariants, implementation latitude, and escalation conditions;
- readiness, open questions, and risk flags when relevant;
- implementation reports when work is submitted;
- reviews;
- final resolution when closed.

Keep long research dumps out of the item body. Put necessary artifacts under the Ticket's `artifacts/` directory and summarize the conclusion in the thread.

Do not store secrets, credentials, private prompt contents, or raw logs containing secrets in Ticket bodies, thread entries, artifacts, diagnostics, or model-visible prompts.

## Backend/maintainer CLI: `yoi ticket`

The product CLI exposes the typed Ticket backend for repository maintenance and validation. It operates on the configured `.yoi/tickets/` storage and is the preferred command-line surface when editing Tickets outside a Worker.

```sh
yoi ticket create --title "..." [--priority P2]
yoi ticket list [--state planning|ready|queued|inprogress|done|closed|all] [--limit n]
yoi ticket show <id>
yoi ticket comment <id> [--role comment|plan|decision|implementation_report] [--file path]
yoi ticket review <id> --approve|--request-changes [--file path]
yoi ticket state <id> <planning|ready|queued|inprogress|done>
yoi ticket close <id> [--resolution text|--file path]
yoi ticket doctor
```

`yoi ticket list` is a capped overview/selection command. It should remain readable for humans and safe for model context: use it to find a canonical id, then use `yoi ticket show <id>` before routing, closing, planning, or implementation decisions.

`yoi ticket state` records current lifecycle transitions among active states. Closing must use `yoi ticket close` so the backend writes the required `resolution.md` and passes `yoi ticket doctor`; `done` and `closed` remain distinct states.

The current LocalTicketBackend stores records under:

```text
.yoi/tickets/<ticket-id>
  item.md
  thread.md
  artifacts/
  resolution.md   # closed Tickets only
```

Backend integrations must preserve this format until an explicit migration changes it. `thread.md` is an append-only typed event log: existing events such as `create`, `comment`, `plan`, `decision`, `implementation_report`, `review`, `state_changed`, and `close` remain valid, while `state_changed` records durable transition metadata (`from`, `to`, `reason`, optional `field`, plus `author` and `at`) and `intake_summary` records the bounded Intake outcome body. Thread events are audit history, not current-state authority; current state belongs in `item.md` frontmatter or the owning backend record. The repository-root `work-items/` path is no longer a live mutable backend; do not recreate it for Ticket records. Human users should prefer `yoi panel`, Ticket tools, or `yoi ticket ...` when working directly with repository records.

## Validation

Run at least:

```sh
yoi ticket doctor
git diff --check
```

Implementation Tickets usually also need focused tests and broader checks, for example:

```sh
cargo fmt --check
cargo check --workspace --all-targets
cargo test -p <crate> <filter>
```

Record validation commands and results in the implementation report or resolution.
