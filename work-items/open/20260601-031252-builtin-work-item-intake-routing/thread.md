<!-- event: create author: tickets.sh at: 2026-06-01T03:12:52Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-01T05:26:27Z -->

## Decision

# WorkItem definition and API shape

## Definition

In Insomnia, a WorkItem is not primarily an issue tracker item. A WorkItem is an agreed execution contract that fixes user intent into a bounded unit that Pods can schedule, delegate, implement, review, and close safely.

Short definition:

> A WorkItem is a durable work contract that preserves user/system-agreed intent and gives Pod groups enough boundary, readiness, and evidence to execute, verify, and complete the work without degrading the original intent.

A ticket file, GitHub Issue, Linear issue, Jira issue, or another tracker record can be a persistence representation of a WorkItem, but none of those representations define the concept itself.

## Roles served by a WorkItem

A WorkItem is simultaneously:

- Intent anchor: preserves what the user and Intake agreed to.
- Delegation packet: lets Orchestrator/Coder understand what should be done and where to stop.
- Review contract: gives Reviewer an explicit basis for approval or request-changes.
- Scheduling unit: lets Orchestrator prioritize, interrupt, parallelize, and assign work.
- Boundary: records scope, non-goals, authority limits, privacy constraints, and escalation conditions.
- Lifecycle record: records plans, decisions, implementation reports, reviews, commits, validation, and resolution.

## Relationship to tickets and issue trackers

- WorkItem is the Insomnia orchestration concept.
- Local markdown ticket is one backend representation.
- GitHub/Linear/Jira issue is another possible backend representation.
- The backend should be abstracted below the WorkItem API.

The WorkItem abstraction should not try to become a complete issue tracker abstraction. It should cover only the subset needed for Insomnia orchestration: create/list/show, structured lifecycle events, status/readiness, review, close, consistency checks, and references/evidence.

## Typing strategy

Use a thin typed envelope with rich Markdown/freeform bodies and events.

The API should type fields that the system reads mechanically, while leaving the content that carries human intent as Markdown/freeform text. This keeps compatibility with external issue trackers and avoids turning Intake into a rigid form-filling flow.

### Typed fields

These should be typed because Orchestrator, TUI, policy, or tools branch on them:

- Identity:
  - backend id
  - external/work item id
- Classification:
  - kind
  - priority
  - labels as free strings
- State:
  - canonical status
  - readiness
  - action-required state
  - needs-preflight flag
  - risk flags
- Lifecycle operation/event kind:
  - comment
  - plan
  - decision
  - implementation report
  - review approved
  - review request-changes
  - status changed
  - closed
- References:
  - related WorkItems
  - files
  - branches
  - commits
  - Pods
  - URLs/artifacts
- Backend capability metadata:
  - create/comment/review/close support
  - artifact support
  - offline support
  - transactional/local-git support

Enums should not be closed too tightly. Use escape hatches such as `Other(String)` or canonical+raw mappings for tracker compatibility.

Examples:

```rust
enum WorkItemKind {
    Bug,
    Task,
    Feature,
    Design,
    Chore,
    Other(String),
}

struct MappedStatus {
    canonical: WorkItemStatus,
    raw: Option<String>,
}
```

### Freeform / Markdown fields

These should remain Markdown/freeform because AI agents and humans can read them well and external trackers can round-trip them:

- issue / problem statement
- user intent
- background / rationale
- requirements
- acceptance criteria
- non-goals
- invariants
- escalation conditions
- investigation notes
- implementation reports
- review comments
- resolution details

The event kind should be typed, but event body should remain Markdown.

```rust
struct WorkItemEvent {
    kind: WorkItemEventKind,
    body: MarkdownText,
    refs: Vec<WorkItemRef>,
}
```

## Suggested initial model

```rust
struct WorkItem {
    id: WorkItemId,
    title: String,
    classification: WorkItemClassification,
    state: WorkItemState,
    body: WorkItemDocument,
    events: Vec<WorkItemEvent>,
    refs: Vec<WorkItemRef>,
    backend: BackendMetadata,
    extensions: serde_json::Map<String, serde_json::Value>,
}
```

```rust
struct WorkItemState {
    status: MappedStatus,
    readiness: Readiness,
    action_required: Option<ActionRequired>,
    needs_preflight: bool,
    risk_flags: Vec<RiskFlag>,
}
```

```rust
struct WorkItemDocument {
    issue: MarkdownText,
    requirements: MarkdownText,
    acceptance_criteria: MarkdownText,
    non_goals: Option<MarkdownText>,
    notes: Option<MarkdownText>,
}
```

## API principle

Operations should be typed; payload bodies should be flexible.

Examples:

- `CreateWorkItem(NewWorkItem)` is typed.
- `AddWorkItemEvent { kind, body }` is typed by operation/event kind, freeform in body.
- `UpdateWorkItemState { readiness, action_required, risk_flags }` is typed.
- `CloseWorkItem { resolution: MarkdownText }` has typed operation semantics and freeform resolution text.

This gives enough structure for audit, scheduling, TUI display, and automation, without forcing issue-tracker-specific fields into the core model.

## Lint over hard schema

Prefer a WorkItem linter/policy layer over strict structural enforcement for body content.

Examples:

- title must be present.
- issue/requirements should not be empty.
- P0/P1 items without acceptance criteria should warn.
- risk flags with `needs_preflight = false` should warn.
- secret-like literals should warn or fail depending on policy.
- closing without review/resolution can warn or fail depending on project policy.

This makes the system agent-friendly and backend-compatible while still catching low-quality WorkItems.

## Backend abstraction

Backends should implement WorkItem storage/sync, not redefine WorkItem semantics.

```text
Intake / Orchestrator / TUI / Workflows
        ↓
    WorkItem API
        ↓
WorkItemBackend
   ├─ LocalFilesBackend
   ├─ GitHubIssuesBackend
   ├─ LinearBackend
   ├─ JiraBackend
   └─ MCPWorkItemBackend
```

Each backend should expose capabilities. The tool/TUI surface should show only operations supported by the configured backend.

## Scope and authority

WorkItem tool authority is separate from normal code/worktree filesystem scope.

Filesystem scope protects exclusive editing of implementation areas. WorkItem management is shared project coordination state and should be mediated by typed WorkItem operations, backend locks/conflict handling, and audit records rather than ordinary delegated write scope.

This does not imply unlimited file access. The WorkItem tool should be limited to configured WorkItem backends and operations.

## Design consequence

The first implementation can be LocalFilesBackend over current `work-items/`, preserving markdown/frontmatter/thread/artifacts and `tickets.sh` compatibility. The API should already be backend-shaped so GitHub Issues or other trackers can be added later without changing Intake/Orchestrator semantics.


---

<!-- event: decision author: hare at: 2026-06-03T12:25:05Z -->

## Decision

# Decision: split feature registry and Hook hardening from Plugin architecture

The Plugin architecture ticket remains the broad architecture surface for Tools, Hooks, runtime kinds, capability model, trust model, discovery/enablement, and MCP/WASM/declarative runtime mapping.

Two implementation-oriented prerequisite tickets are split out:

- `plugin-feature-contribution-registry`: define and implement the Pod-layer feature contribution registry so built-in and future external capabilities register through existing Tool / Hook / Notify paths instead of ad hoc Pod code paths.
- `hook-public-surface-hardening`: audit and harden `pod::hook` before exposing it as a feature/plugin contribution boundary, especially removing public access to raw internal action types that can inject model-visible `Item` values.

This preserves the desired detachable shape: feature state remains in the feature/extension module, while Pod interaction happens through existing durable host surfaces. WorkItem management should be implemented as a built-in feature contribution once the registry boundary is in place, rather than as a special Pod context-injection path.


---

<!-- event: decision author: hare at: 2026-06-05T04:03:37Z -->

## Decision

Decision: use `Ticket` as the durable orchestration record concept.

Rationale:

- `WorkItem` is not a preferred product/code name.
- `Work*` naming feels too enterprise/work-order-like for this system.
- `Issue` overemphasizes problems/bugs and does not fit design, cleanup, investigation, and orchestration records as well.
- `Task` is already used for session-local progress tracking and implies a smaller unit.
- `Ticket` is broad enough for requests, bugs, design work, implementation tasks, reviews, spikes, and orchestration epics while remaining natural in the current repository workflow.

Terminology:

- `Ticket` is the durable orchestration record.
- `Task` remains session-local progress tracking.
- `Assignment` is a concrete delegation from Orchestrator to a coder/reviewer/investigator Pod.
- `IntentPacket` is a short implementation/review contract derived from a Ticket.
- `LocalTicketBackend` is the current `work-items/` markdown/thread/artifact storage backend.

Scope decision:

- Keep `work-items/` as the current storage path for now.
- Do not rename `work-items/` in this phase.
- Treat the old WorkItem wording as historical terminology being replaced by Ticket.


---

<!-- event: plan author: hare at: 2026-06-05T04:03:37Z -->

## Plan

Plan: split the original broad built-in intake/routing ticket into implementation-sized Ticket tickets.

The parent ticket is now an umbrella for the Ticket-driven intake/orchestration path. The implementation sequence is:

1. `ticket-local-files-backend`
   - Add the code-facing Ticket domain/backend layer and LocalTicketBackend over current `work-items/` files.

2. `ticket-built-in-feature-tools`
   - Expose typed Ticket operations as a built-in Pod feature/tool surface with Ticket backend authority, not arbitrary filesystem write scope.

3. `ticket-intake-workflow`
   - Add the Intake workflow/profile that clarifies user intent and creates/updates Tickets after user agreement.

4. `ticket-orchestrator-routing`
   - Add explicit Orchestrator routing classification and intent-packet production for Tickets.

`tui-spawned-pod-panel` is not part of this path; it may be useful UI work later, but it is not what is currently needed for the multi-agent system.


---

<!-- event: comment author: hare at: 2026-06-05T04:04:42Z -->

## Comment

Added current terminology/design artifact:

- `artifacts/ticket-definition-and-api-shape-20260605.md`

The earlier `workitem-definition-and-api-shape-20260601.md` is now explicitly marked superseded and retained only as historical context.


---
