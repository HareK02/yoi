# Ticket definition and API shape

## Definition

In yoi, a Ticket is the durable orchestration record shared by humans, Intake Pods, Orchestrator Pods, coder Pods, and reviewer Pods.

Short definition:

> A Ticket is a durable coordination contract that preserves agreed intent, requirements, readiness, decisions, evidence, review results, and resolution history so Pod groups can route, delegate, implement, verify, and complete work without losing the original intent.

A local markdown ticket, GitHub Issue, Linear issue, Jira issue, or another tracker record can be a backend representation of a Ticket, but none of those storage representations define the concept.

The current local backend is the repository's `work-items/` directory managed by `tickets.sh`. That path remains unchanged for now.

## Roles served by a Ticket

A Ticket is simultaneously:

- Intent anchor: preserves what the user and Intake agreed to.
- Requirements contract: states observable requirements, acceptance criteria, non-goals, and escalation conditions.
- Routing unit: lets Orchestrator decide between requirements sync, preflight, spike, implementation, review, blocked/action-required, pending, or close.
- Delegation source: lets Orchestrator derive an `IntentPacket` for a concrete Pod `Assignment`.
- Review contract: gives reviewer Pods an explicit basis for approval or request-changes.
- Evidence ledger: records plans, decisions, implementation reports, validation, review, artifacts, commits, branches, and resolution.
- Backend-neutral record: local files are the first backend, not the product concept.

## Relationship to other terms

- `Task`: session-local progress tracking inside a Pod/session. Smaller and more ephemeral than a Ticket.
- `Ticket`: durable project/orchestration record.
- `Assignment`: concrete delegation from Orchestrator to a coder/reviewer/investigator Pod.
- `IntentPacket`: concise implementation/review contract extracted from a Ticket and passed to an Assignment.
- `TicketBackend`: backend abstraction for Ticket storage/mutation.
- `LocalTicketBackend`: current local file backend over `work-items/`.

## Conceptual shape

The Rust model should use a thin typed envelope around Markdown/freeform body content.

Typed enough for machines:

- id / slug / title;
- kind / priority / labels;
- status;
- readiness;
- needs-preflight;
- risk flags;
- action-required state;
- event kind;
- review result;
- artifact references;
- branch/commit/Pod references where useful.

Flexible enough for humans/LLMs:

- background;
- requirements;
- acceptance criteria;
- rationale;
- design notes;
- plan;
- implementation report;
- review body;
- resolution.

## Candidate types

```rust
struct Ticket {
    id: TicketId,
    slug: TicketSlug,
    title: String,
    meta: TicketMeta,
    body: TicketDocument,
    events: Vec<TicketEvent>,
    artifacts: Vec<TicketArtifactRef>,
}

struct TicketSummary {
    id: TicketId,
    slug: TicketSlug,
    title: String,
    status: TicketStatus,
    kind: TicketKind,
    priority: TicketPriority,
    readiness: TicketReadiness,
    needs_preflight: bool,
    action_required: Option<ActionRequired>,
}

enum TicketStatus {
    Open,
    Pending,
    Closed,
    Other(String),
}

enum TicketReadiness {
    Unspecified,
    RequirementsSyncNeeded,
    SpikeNeeded,
    ImplementationReady,
    Blocked,
    Other(String),
}

enum TicketEventKind {
    Comment,
    Plan,
    Decision,
    ImplementationReport,
    Review,
    StatusChanged,
    Closed,
    Other(String),
}

struct TicketEvent {
    kind: TicketEventKind,
    body: MarkdownText,
    refs: Vec<TicketRef>,
}
```

## Candidate backend trait

```rust
trait TicketBackend {
    fn list(&self, filter: TicketFilter) -> Result<Vec<TicketSummary>>;
    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket>;
    fn create(&self, input: NewTicket) -> Result<TicketRef>;
    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()>;
    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()>;
    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
}
```

The trait should remain backend-shaped and should not bake local file paths into the concept. Local paths belong in `LocalTicketBackend` configuration and implementation.

## Authority boundary

Ticket authority is separate from ordinary source/worktree filesystem scope.

Filesystem scope protects implementation areas and source edits. Ticket management is shared project coordination state and should be mediated through typed Ticket operations, backend locks/conflict handling, and audit/history records rather than delegated arbitrary write access.

This does not imply unlimited Ticket access. Ticket tools should be granted explicitly and limited to configured Ticket backends and operations.

## Backend layering

```text
Ticket API / TicketBackend
  ├─ LocalTicketBackend   -> current work-items/ directory
  ├─ GitHubTicketBackend  -> possible future backend
  ├─ LinearTicketBackend  -> possible future backend
  └─ McpTicketBackend     -> possible future bridge
```

Local files are the first backend because they are already the authoritative project record in this repository. External tracker support is not part of the MVP.

## Validation/linting direction

Prefer a Ticket linter/policy layer over strict structural enforcement for all body content.

The code should enforce mechanical consistency and safe mutation:

- required frontmatter fields;
- status directory consistency;
- id/slug uniqueness;
- thread event parseability;
- review/result shape;
- resolution existence for closed tickets;
- artifact path containment;
- lock/conflict behavior;
- bounded diagnostics.

The code should not require every Ticket body to fit a rigid universal schema. Ticket bodies remain Markdown/freeform because design, cleanup, bugs, investigations, and orchestration epics need different prose.

## Current implementation sequence

1. `ticket-local-files-backend`
2. `ticket-built-in-feature-tools`
3. `ticket-intake-workflow`
4. `ticket-orchestrator-routing`
