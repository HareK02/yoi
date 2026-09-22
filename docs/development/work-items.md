# Tickets and development workflow

Yoi project work is tracked through Tickets. For normal use, interact with Tickets through `yoi panel`, Ticket tools, the `yoi ticket ...` CLI, and typed role surfaces. Workspace Server control-plane records plus the append-only Ticket event and Merge Request evidence streams are authoritative.

Ticket and Objective records are stored in the Workspace Server database. Repository-local `.yoi/tickets`, `.yoi/objectives`, and config files are ignored legacy input, not a debugging or compatibility backend.

Do not treat ad-hoc chat summaries, memory records, or Worker notifications as the final source of project state. Notifications are hints to inspect concrete state, not proof of completion.

## Concepts

- `Ticket`: durable project/orchestration record. It contains requirements, decisions, plans, implementation reports, reviews, artifacts, and resolution history.
- `Objective`: first-class medium-term goal record stored by the Workspace Server. It stores goal, motivation/background, strategy/design direction, success criteria/exit conditions, decision context, current Objective lifecycle, and canonical Ticket links. Objective context is judgment/background context; it is not implementation authority and does not replace reading each Ticket body, thread, and evidence.
- `Task`: session-local progress tracking inside a Worker. It is not the project record.
- `Assignment`: a concrete delegation from an Orchestrator to a coder/reviewer Worker or task-specific helper Worker.
- `IntentPacket`: the short implementation/review contract derived from a Ticket and handed to an Assignment.
- `Ticket backend`: the Workspace Server control-plane database exposed through typed Workspace APIs.
- `Ticket relation`: durable project-level Ticket-to-Ticket metadata stored as forward canonical-id relations (`depends_on`, `blocks`, `related`, `supersedes`, `duplicate_of`). Inverse views such as `blocked_by` are derived, not stored.

A Ticket may represent a feature, bug, cleanup, design decision, investigation, workflow change, release task, or orchestration task. The common requirement is that the Ticket is a concrete work item that can be implemented, reviewed, validated, and closed on its own terms.

## User-facing entry points

Use the highest-level interface that matches the work:

- Use `yoi panel` for the Ticket/Intake/Orchestrator workspace Dashboard and role-launch actions.
- Use `yoi objective ...` for lightweight medium-term Objective records and their non-blocking canonical Ticket links.
- Inside Workers, use typed Ticket tools for Ticket records and typed Merge Request tools for immutable implementation/review/completion evidence.
- For multi-step work, follow the typed Ticket role surfaces and recorded Ticket lifecycle gates.

Maintainers inspect Ticket state through the authenticated Workspace API, Server administration surfaces, and database diagnostics. Repository-local Ticket files are not storage authority.

## Ticket tools inside Workers

Workers with the Ticket and operation-specific Merge Request built-in features can use typed workflow tools:

- `TicketCreate`
- `QueryTicket` — bounded authoritative Ticket discovery with typed state/text/event/evidence/relation/Objective/time/attention filters, stable snippets, and cursor metadata.
- `ShowTicket` — detailed authority for one Ticket, including item revision, bounded thread/event references, relations, linked Objectives, implementation reports, and current Merge Request/review evidence.
- `TicketComment`
- Coder: `ShowMergeRequest`, `OpenMergeRequest`
- Reviewer: `ShowMergeRequest`, `ReviewMergeRequest` — available only inside the attested direct-child Reviewer request; grant and subject-ref capability material are not model input.
- Orchestrator: `ShowMergeRequest`, `CheckMergeRequestReadiness`, `CompleteMergeRequest`
- `TicketClose`
- `TicketRelationRecord`

Profile-visible Ticket catalogs are intentionally smaller than the former broad read catalog: Workspace authoring exposes 9 tools instead of 13, workflow exposes 10 instead of 12, and review exposes only `QueryTicket` plus `ShowTicket` (2 instead of 6). The `QueryTicket` schema is regression-guarded below 8 KiB while consolidating relation/evidence/attention discovery; diagnostics are not projected into normal profiles, while specialized orchestration-plan commands remain visible only to workflow roles that need their distinct semantics.

These tools operate through the typed Workspace Ticket API. They do not grant arbitrary filesystem access and never select repository-local Ticket storage.

Relation tools are for non-hierarchical project metadata only. Use canonical opaque Ticket ids, store forward relations only, and keep runtime execution planning (capacity, ordering decisions, do-not-parallelize notes, Worker/session/worktree ownership) in OrchestrationPlan or session-local records instead of relation metadata. Unresolved `depends_on` and incoming unresolved `blocks` are queue/acceptance blockers; `related` is not blocking, and `supersedes` / `duplicate_of` are diagnostics rather than automatic lifecycle transitions.

Use them when a Worker needs to materialize or update project records:

- Intake creates a new Ticket after user agreement.
- Orchestrator records routing decisions and intent packets.
- Reviewer commits an approve/request-changes result against one immutable Merge Request revision.
- Maintainer closes a Ticket with a resolution when merge/validation/cleanup evidence is complete.

Do not bypass Ticket lifecycle gates just because Ticket tools are available. Ticket mutation is a project-record operation and should remain auditable.

## Objective records

Objectives are Workspace Server control-plane records, not files in a checkout. Use typed Objective tools or `yoi objective ...`; both resolve the selected Backend/Workspace through the shared client target and call the Workspace API.

Objective-to-Ticket links are context links only: they are not dependency, blocking, ordering, ownership, or scheduling relations. Objective lifecycle does not drive Ticket state or authorize implementation. A role reading Objective context must still inspect each Ticket and its current Merge Request evidence.

Repository-local `.yoi/objectives` trees are ignored. There is no automatic import or cwd/ancestor fallback.

## Ticket configuration

Workspace Ticket data and workflow authority live in the Workspace Server's SQLite control-plane store. Repository-local `.yoi/workspace.toml` and `.yoi/ticket.config.toml` are not Ticket, Workspace identity, Backend connection, or role-launch authority. `yoi init --display-name <NAME> --repository-key <KEY>` registers the current Git repository through the Backend API and writes only global client routing under `$XDG_CONFIG_HOME/yoi/client.toml`.

Fixed Ticket workflow roles are `intake`, `orchestrator`, `coder`, and `reviewer`. The Server resolves the selected Profile and launch material from the active Workspace configuration authority, and Runtime receives the resulting immutable launch snapshot. A repository checkout may still contain ordinary project files, but neither the client nor Runtime may infer Workspace identity, Backend routing, role Profile, or Ticket storage from repository-local `.yoi` files.

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

The assigned Coder launches the Reviewer as an actual direct-child `builtin:reviewer` SubWorker with write scope, so it can use the Workdir command tools required for inspection and validation, and a structured handoff bound to the current immutable Merge Request revision. Server authority revalidates the parent assignment, Runtime-owned child session, effective profile, one-shot review attempt, and revision; prose output is not approval.

The Reviewer records the structured result with `ReviewMergeRequest`. Request changes advances the existing Merge Request source selector with a normal non-force push and requires a fresh child attempt for that exact new source ref; do not create a replacement Merge Request, add-revision operation, or fresh integration branch. Target-only movement preserves approval for an unchanged source and requires refreshed integration evidence. The Orchestrator uses `CheckMergeRequestReadiness` and then `CompleteMergeRequest` for guarded integration with operation-id dedupe/CAS semantics; Flow transitions are not completion authority.

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

Role actions map to the fixed Workspace Ticket roles:

- intake launches the intake role without an existing Ticket and requires freeform context.
- route launches the orchestrator role for an existing Ticket.
- implement launches the coder role for an implementation assignment.
- review launches the reviewer role for review.

All actions are explicit and user-triggered. They are not a scheduler, queue, spawned-Worker Dashboard, or automatic maintainer loop.

### Dashboard execution path

The Dashboard sends the selected action and Ticket context to the Workspace Server. The Server validates Workspace access, resolves the current Server DB Workspace/Ticket authority and active Profile projection, launches or restores the role Worker through the shared Worker path, commits the typed initial input, and returns durable acceptance evidence. The client does not inspect repository-local `.yoi` files, choose a Ticket storage directory, or construct Runtime launch authority.

The launched Worker receives dynamic Ticket/action context as its first committed run input. The selected Profile supplies durable system/role behavior. Workspace Ticket metadata does not override system instruction.

### Dashboard troubleshooting

- unresolved Workspace selection: select an accessible Workspace from the Server catalog or pass an explicit Workspace selector.
- unavailable role Profile: update the active Workspace configuration and retry after the Server projects the new revision.
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

Keep long research dumps out of the item body. Attach necessary bounded artifacts through the Ticket evidence surface and summarize the conclusion in the thread.

Do not store secrets, credentials, private prompt contents, or raw logs containing secrets in Ticket bodies, thread entries, artifacts, diagnostics, or model-visible prompts.

## Backend CLI: `yoi ticket`

The product CLI resolves an explicit or configured Backend target and performs Ticket operations through the Workspace API. It does not open a repository-local backend. Use command help for the current typed command surface.

Legacy `ticket init` and `ticket import-local` workflows are unsupported. No automatic migration reads old `.yoi/tickets` trees; export data with an older version before upgrading if it must be retained, then import it through an explicit supported Workspace surface.

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
