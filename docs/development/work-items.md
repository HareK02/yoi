# Tickets and development workflow

Yoi project work is tracked through Tickets. For normal use, interact with Tickets through `yoi panel`, Ticket tools, the `yoi ticket ...` CLI, and Ticket workflows. Git history plus Ticket files remain the authoritative state-transition record behind those interfaces.

The current local backend stores Ticket files under `.yoi/tickets/`. That storage detail matters for maintainers and backend compatibility, but it is not the primary user-facing workflow.

Do not treat ad-hoc chat summaries, memory records, or Pod notifications as the final source of project state. Notifications are hints to inspect concrete state, not proof of completion.

## Concepts

- `Ticket`: durable project/orchestration record. It contains requirements, decisions, plans, implementation reports, reviews, artifacts, and resolution history.
- `Objective`: medium-term goal, motivation, strategy, and success-criteria context. Objective records are a policy concept here; this document does not define their storage/schema.
- `Task`: session-local progress tracking inside a Pod. It is not the project record.
- `Assignment`: a concrete delegation from an Orchestrator to a coder/reviewer Pod or task-specific helper Pod.
- `IntentPacket`: the short implementation/review contract derived from a Ticket and handed to an Assignment.
- `LocalTicketBackend`: the current `.yoi/tickets/` markdown/thread/artifacts storage backend.

A Ticket may represent a feature, bug, cleanup, design decision, investigation, workflow change, release task, or orchestration task. The common requirement is that the Ticket is a concrete work item that can be implemented, reviewed, validated, and closed on its own terms.

## User-facing entry points

Use the highest-level interface that matches the work:

- Use `yoi panel` for the Ticket/Intake/Orchestrator workspace UI and role-launch actions.
- Inside Pods, use typed Ticket tools to create, inspect, comment, review, and close Tickets.
- For multi-step work, follow the Ticket Intake, Orchestrator Routing, planning/requirements-sync, and Multi-agent workflows.

Maintainers can inspect the local `.yoi/tickets/` files directly when debugging storage, but normal user instructions should go through `yoi panel`, Ticket tools, or `yoi ticket ...`.

## Ticket tools inside Pods

Pods with the Ticket built-in feature can use typed Ticket tools:

- `TicketCreate`
- `TicketList`
- `TicketShow`
- `TicketComment`
- `TicketReview`
- `TicketStatus`
- `TicketClose`
- `TicketDoctor`

These tools operate through the typed Ticket backend. They are not arbitrary filesystem write permission to `.yoi/tickets/`.

Use them when a Pod needs to materialize or update project records:

- Intake creates a new Ticket after user agreement.
- Orchestrator records routing decisions and intent packets.
- Reviewer records approve/request-changes review results.
- Maintainer closes a Ticket with a resolution when merge/validation/cleanup evidence is complete.

Do not bypass workflow gates just because Ticket tools are available. Ticket mutation is a project-record operation and should remain auditable.

## Ticket configuration

Workspace Ticket orchestration is configured by `.yoi/ticket.config.toml` when present.

MVP shape:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"

[roles.intake]
profile = "project:intake"
launch_prompt = "$workspace/ticket/intake/launch"
workflow = "ticket-intake-workflow"

[roles.orchestrator]
profile = "project:orchestrator"
launch_prompt = "$workspace/ticket/orchestrator/launch"
workflow = "ticket-orchestrator-routing"

[roles.coder]
profile = "project:coder"
launch_prompt = "$workspace/ticket/coder/launch"
workflow = "multi-agent-workflow"

[roles.reviewer]
profile = "project:reviewer"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "multi-agent-workflow"
```

Fixed roles are:

- `intake`
- `orchestrator`
- `coder`
- `reviewer`

This is not an arbitrary role registry. The fixed roles are the roles required by Ticket orchestration.
Stale `[roles.investigator]` config is rejected as an unsupported fixed role; remove it and,
when a spike is useful, let the Orchestrator create an ordinary task-specific read-only helper Pod.

`profile` selects the Pod runtime Profile for that role. The selected Profile owns durable role/system behavior. `ticket.config.toml` does not have a role-level `system_instruction` field.

`launch_prompt` is a per-action first-run prompt reference for future prompt resolution. Current launcher behavior exposes the ref but does not treat it as system instruction.

`workflow` is the workflow the launched role should follow. Workflow state and phase-specific prompt injection are future work; any dynamic prompt content must be committed as history before it affects model context.

`provider = "builtin:yoi_local"` selects Yoi's built-in local Ticket backend. `root = ".yoi/tickets"` is the canonical local storage root for this repository. Legacy `kind = "local"` is accepted only as a short transitional alias; new configs should use `provider`.

If `.yoi/ticket.config.toml` is missing, defaults are:

- backend provider: `builtin:yoi_local`
- backend root: `<workspace>/.yoi/tickets`
- all role profiles: `inherit`
- no launch prompt refs
- workflows:
  - intake: `ticket-intake-workflow`
  - orchestrator: `ticket-orchestrator-routing`
  - coder: `multi-agent-workflow`
  - reviewer: `multi-agent-workflow`

Important: top-level Ticket role launches cannot execute `profile = "inherit"` because top-level launch has no parent Profile to inherit from. Configure concrete role profiles in `.yoi/ticket.config.toml` before using `yoi panel` role-launch actions.

## Workflow lifecycle

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

Use `ticket-intake-workflow` when a user request is broad, ambiguous, or not yet a Ticket.

Intake should:

- clarify user intent;
- check duplicate/related Tickets;
- draft background, requirements, acceptance criteria, binding decisions/invariants, implementation latitude, readiness, risk flags, and validation;
- create or update the Ticket only after user agreement.

Intake should not schedule implementation, spawn coder/reviewer Pods, create worktrees, merge, or close Tickets.

### 2. Orchestrator routing

Use `ticket-orchestrator-routing` to classify the next action for an existing Ticket.

Routing classifications include:

- `requirements_sync_needed`
- `return_to_planning`
- `spike_needed`
- `implementation_ready`
- `review_needed`
- `blocked_action_required`
- `close_ready`
- `defer_pending`
- `closed_or_noop`

Routing decisions should be recorded with `TicketComment` using `plan` or `decision` role. The decision should state the classification, evidence checked, reason, next action, and escalation conditions. For `return_to_planning`, the record must also state the concrete missing decision/information, context checked, why implementation latitude is insufficient, and the next planning question/action.

### 3. Planning/requirements sync

Use `ticket-preflight-workflow` only as a legacy compatibility slug for planning/requirements sync. Return `ready` or `queued` Tickets to `planning` only when the Orchestrator can name a concrete missing decision or information item after bounded project-context checks; risk flags and risky domains are context-lookup and reviewer-focus signals, not automatic stop gates.

Planning sync should resolve or record:

- requirements and acceptance criteria;
- current code map;
- binding decisions/invariants and implementation latitude;
- critical risks and failure modes;
- implementation-ready vs requirements-sync/spike/blocked classification.

Do not send Tickets with unresolved concrete missing decisions/information directly to coder Pods. If no concrete missing item remains after bounded checks, risky-but-specified Tickets should proceed with an IntentPacket plus escalation conditions and reviewer focus.

### 4. Implementation assignment

Use `multi-agent-workflow` for implementation-ready Tickets.

The Orchestrator should prepare an `IntentPacket` with:

- intent;
- requirements;
- binding decisions/invariants;
- implementation latitude;
- escalation conditions;
- validation;
- current code map;
- critical risks.

Implementation normally happens in a child git worktree created by the Orchestrator, not by the coder Pod. The coder Pod receives narrow write scope to the worktree and must report changed files, implementation summary, validation, unresolved risks, and review readiness.

### 5. Review

Reviewer Pods should be sibling Pods, not children of coder Pods. They should read the Ticket, intent packet, diff, implementation report, and validation evidence.

Review results should be recorded with the `TicketReview` tool. Maintainers working directly with the local backend can use the `yoi ticket` CLI documented later.

Blockers must be fixed or explicitly escalated before merge-ready submission.

### 6. Merge and close

Unless explicitly authorized otherwise, final merge, cleanup, design-boundary decisions, and Ticket closure remain Orchestrator/human responsibilities.

Before closing, verify concrete evidence:

- child Pod output via `ReadPodOutput`;
- worktree status and diff;
- validation command output;
- review result;
- Ticket requirements and acceptance criteria;
- merge/cleanup state in the main workspace.

Close with a resolution that summarizes what changed, key commits, validation, review status, and remaining follow-ups.

## Workspace panel Ticket role actions

`yoi panel` is the active Ticket/Intake/Orchestrator UI. It owns fixed Ticket role-launch actions and uses the shared client Ticket role launcher. The single-Pod TUI no longer supports `:ticket ...` commands; typing them in command mode is treated like any other unknown command.

Role actions map to the same fixed roles configured in `.yoi/ticket.config.toml`:

- intake launches the intake role without an existing Ticket and requires freeform context.
- route launches the orchestrator role for an existing Ticket.
- implement launches the coder role for an implementation assignment.
- review launches the reviewer role for review.

All actions are explicit and user-triggered. They are not a scheduler, queue, spawned-Pod panel, or automatic maintainer loop.

### Panel execution path

The role-launch path is:

```text
User triggers a Ticket action in yoi panel
  -> panel builds a TicketRoleLaunchContext
  -> client Ticket role launcher reads .yoi/ticket.config.toml
  -> launcher selects the role Profile and workflow
  -> launcher spawns the role Pod
  -> launcher sends Method::Run with WorkflowInvoke + Text segments
  -> launcher waits for run-acceptance evidence
  -> panel reports success/failure
```

The launched Pod receives dynamic Ticket/action context as its first committed run input. The panel does not inject hidden context, does not write Ticket files directly, and does not construct prompt/workflow segments by hand.

The first run input contains:

- the selected fixed role;
- the workflow slug from `.yoi/ticket.config.toml`;
- Ticket id/slug when the action targets an existing Ticket;
- freeform user instruction/context from the action;
- configured `launch_prompt` reference if present, as an unresolved reference for future prompt resolution.

The selected Profile supplies durable system/role behavior. `ticket.config.toml` does not override system instruction.

### Panel setup

Because top-level role launches cannot inherit a parent Profile, configure concrete role profiles before using panel role actions:

```toml
# .yoi/ticket.config.toml

[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"

[roles.intake]
profile = "project:intake"
workflow = "ticket-intake-workflow"

[roles.orchestrator]
profile = "project:orchestrator"
workflow = "ticket-orchestrator-routing"

[roles.coder]
profile = "project:coder"
workflow = "multi-agent-workflow"

[roles.reviewer]
profile = "project:reviewer"
workflow = "multi-agent-workflow"
```

If a role still uses `profile = "inherit"`, the panel fails closed with a diagnostic explaining that a concrete profile is required.

### Panel troubleshooting

- `profile = "inherit"`: configure a concrete role Profile in `.yoi/ticket.config.toml`.
- malformed `.yoi/ticket.config.toml`: fix the config and retry.
- missing Ticket id/slug for route, implement, or review actions: provide the target Ticket.
- launch success but no visible completion: attach to or inspect the launched Pod; completion notifications are hints, not authority.

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

The product CLI exposes the typed Ticket backend for repository maintenance and validation. It operates on the configured `.yoi/tickets/` storage and is the preferred command-line surface when editing Tickets outside a Pod.

```sh
yoi ticket create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]
yoi ticket list [--status open|pending|closed|all]
yoi ticket show <id-or-slug>
yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--file path]
yoi ticket review <id-or-slug> --approve|--request-changes [--file path]
yoi ticket status <id-or-slug> open|pending
yoi ticket close <id-or-slug> [--resolution text|--file path]
yoi ticket doctor
```

`yoi ticket status` intentionally does not close Tickets. Closing must use `yoi ticket close` so the backend writes the required `resolution.md` and passes `yoi ticket doctor`.

The current LocalTicketBackend stores records under:

```text
.yoi/tickets/{open,pending,closed}/<id>/
  item.md
  thread.md
  artifacts/
  resolution.md   # closed Tickets only
```

Backend integrations must preserve this format until an explicit migration changes it. `thread.md` is an append-only typed event log: existing events such as `create`, `comment`, `plan`, `decision`, `implementation_report`, `review`, `status_changed`, and `close` remain valid, while `state_changed` records durable transition metadata (`from`, `to`, `reason`, optional `field`, plus `author` and `at`) and `intake_summary` records the bounded Intake outcome body. Thread events are audit history, not current-state authority; current state belongs in `item.md` frontmatter or the owning backend record. The repository-root `work-items/` path is no longer a live mutable backend; do not recreate it for Ticket records. Human users should prefer `yoi panel`, Ticket tools, or `yoi ticket ...` when working directly with repository records.

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
nix build .#yoi --no-link
```

Record validation commands and results in the implementation report or resolution.
