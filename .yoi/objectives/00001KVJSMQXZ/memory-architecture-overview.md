---
created_at: "2026-07-15T21:33:00Z"
objective: "00001KVJSMQXZ"
status: "architecture-draft"
notes: "Draft architecture for redesigning Yoi Memory as a sensemaking substrate. This is an Objective resource, not implementation authority."
---

# Memory architecture overview

## Position

Yoi Memory should be redesigned as a **sensemaking substrate** rather than a larger persistent note store. The core architecture should help an agent gather task-relevant material, preserve evidence and provenance, build intermediate representations, test hypotheses, and feed outcomes back into Tickets, reviews, docs, Skills, and implementation decisions.

This document intentionally assumes the current Memory/Knowledge implementation can be redesigned. Existing `summary` / `decision` / `request` records, extraction, consolidation, resident context, and memory tools are useful historical inputs, but they should not constrain the target model when they conflict with the sensemaking architecture.

## Design constraints

- Memory is not project authority.
  - Tickets, docs, git history, session logs, explicit user instructions, Workspace records, and typed feature/tool results remain authority.
  - Memory provides evidence indexes, working sets, synthesis, reminders, and candidate updates.
- Knowledge is not a separate target record kind.
  - Reusable procedures belong in Skills.
  - Maintained policies and design rationale belong in docs, Ticket decisions, Objective resources, or explicit Memory decisions depending on authority.
- Workflow tracking is not part of Memory.
  - Procedure guidance belongs in Skills / role prompts.
  - External state transitions belong in typed feature/tool surfaces.
- Context-only injection is forbidden.
  - Any new model-visible memory/shoebox/evidence context must be appended to Worker history or represented through explicit tool results/artifacts.
- Workspace backend should become the shared authority for Memory-related control-plane APIs.
  - Local `.yoi/memory` can remain compatibility/offline storage while the architecture is proven.

## Reference model

The target architecture follows Pirolli & Card's sensemaking process:

```text
External data sources
  -> task-bound shoebox
  -> evidence file
  -> schemas / representations
  -> hypotheses
  -> product
  -> product feedback / memory maintenance
```

Yoi should optimize the loop, not just the final durable storage.

## Core concepts

### 1. External sources

External sources are authoritative or semi-authoritative material that Memory can point to but should not replace.

Examples:

- Ticket item/thread/resolution/artifacts.
- Objective item/resources.
- Git commits, diffs, branches, worktrees.
- Session logs and Worker transcripts.
- Docs and reports.
- Code references.
- Runtime/Workspace API records.
- Explicit user messages.
- Existing Memory records.
- Skill metadata and `SKILL.md` content when procedural context matters.

External sources should be addressed through source refs, not copied wholesale into Memory.

### 2. Task-bound shoebox

A shoebox is a bounded working set of potentially relevant material for a concrete question, Ticket, Objective, review, or design task.

Properties:

- Scoped to a task/question.
- Contains source refs plus short rationale for inclusion.
- Can include both likely supporting and likely contradicting material.
- Does not assert conclusions.
- Is disposable or artifact-like; it is not necessarily durable memory.

Example uses:

- Orchestrator asks for context before routing a queued Ticket.
- Reviewer asks for prior decisions and contradictory evidence for a change.
- Designer asks for relevant reports, tickets, commits, and old memory before writing architecture.

A shoebox is the first concrete slice to prototype because it directly addresses Memory graveyard behavior: information must gather around the current question.

### 3. Evidence file

An evidence file extracts snippets or observations from a shoebox.

Each evidence item should carry:

- source ref;
- quoted or summarized snippet;
- source location/anchor if available;
- why it matters;
- supports / refutes / contextualizes relation;
- applicability scope;
- confidence;
- staleness or supersession markers;
- extractor identity/time.

Evidence files are not final decisions. They are structured working material for reasoning.

### 4. Representation / schema layer

The representation layer reorganizes evidence into forms that reduce reasoning cost.

Possible representations:

- timeline;
- subsystem map;
- authority boundary map;
- invariant list;
- risk list;
- decision table;
- hypothesis table;
- alternative/rejected alternative table;
- contradiction/staleness report;
- review checklist;
- dependency graph.

Representations should be attached to a task or Objective as artifacts/resources before becoming durable Memory records.

### 5. Hypotheses and alternatives

Memory should help track not only final conclusions but also competing interpretations.

A hypothesis record or artifact should be able to express:

- claim;
- supporting evidence;
- contradicting evidence;
- open questions;
- rejected alternatives;
- decision threshold;
- current status: proposed / accepted / rejected / stale / superseded.

This is the main anti-confirmation-bias layer. Reviewer and Orchestrator Skills should explicitly ask agents to seek disconfirming evidence, but the data structure should make that evidence easy to preserve.

### 6. Product feedback

The loop must end in a product. A memory-driven activity should normally produce or update one of:

- Ticket decision/comment/implementation report/review;
- Objective resource;
- design doc/report;
- Skill update;
- code implementation or validation evidence;
- explicit Memory decision/request update;
- stale/superseded marker on old memory.

Memory that is collected but never changes a product should be considered low-value unless it is intentionally kept as a temporary shoebox/evidence artifact.

## Storage layers

### Compatibility layer: current `.yoi/memory`

Keep only the minimum durable Memory record kinds needed during transition:

- summary;
- decision;
- request;
- audit/extraction logs as implementation artifacts.

Do not reintroduce Knowledge as a separate kind.

Existing generated Memory can continue to serve as background context, but new architecture should not optimize around resident memory injection first.

### Workspace control-plane layer

Target authority should move toward Workspace backend APIs for:

- memory catalog/search;
- task shoebox creation/list/show;
- evidence file creation/list/show;
- artifact/source refs;
- staleness/supersession markers;
- product-impact metrics;
- memory maintenance diagnostics.

This mirrors the direction already chosen for Tickets and Skills: Workers should not maintain divergent local interpretations when `WorkspaceClient::Http` is available.

### Artifact/resource layer

Before inventing new durable schemas, prefer Objective/Ticket resources or artifacts for prototypes:

```text
.yoi/objectives/<objective-id>/resources/<name>.md
.yoi/tickets/<ticket-id>/artifacts/<name>.json|md
```

This keeps early experiments inspectable and avoids overfitting storage before the architecture is proven.

## Runtime/API architecture

### Memory service responsibilities

A future Workspace Memory service should provide typed operations for:

- `MemorySourceSearch`: find candidate source refs across Tickets, Objectives, docs, sessions, code, reports, and existing Memory.
- `ShoeboxCreate`: create a task-bound working set from query/task/source refs.
- `ShoeboxShow`: render a bounded shoebox for an agent.
- `EvidenceExtract`: extract evidence items from a shoebox or selected source refs.
- `EvidenceShow`: render evidence with provenance and support/refute relations.
- `RepresentationCreate`: create tables/maps/timelines/hypothesis matrices from evidence.
- `MemoryCandidatePropose`: propose durable Memory/doc/Ticket/Skill updates based on artifacts.
- `MemoryImpactRecord`: record when memory/evidence influenced a product.
- `MemoryStalenessMark`: mark records as stale/superseded/contradicted/needs-review.

These should be typed feature/tool surfaces, not hidden prompt injection.

### Worker interaction

Workers should interact with Memory through explicit tools/API results:

- A Worker asks for a shoebox or evidence file.
- The result is committed as a tool result or artifact reference.
- If the Worker needs additional source material, it explicitly reads/fetches it.
- If a result should influence future work, the Worker writes a Ticket comment, Objective resource, doc update, Skill update, or Memory candidate.

Resident context may still include high-signal summary, but it should not be the primary product of the Memory system.

### Prompt/context behavior

- Shoebox and evidence outputs should be bounded.
- Large source material should remain referenced, not inlined.
- If a generated summary or evidence bundle is used for reasoning, it must be visible in history/tool output/artifact.
- Context should distinguish source quote, extractor summary, and model inference.

## Metrics

Memory metrics should distinguish exposure from impact.

Low-level events:

- resident exposure;
- explicit query;
- shoebox created;
- evidence extracted;
- representation created;
- source opened/read;
- stale marker created.

Product-impact events:

- cited in Ticket comment/review/resolution;
- changed implementation plan;
- changed review outcome;
- changed acceptance criteria or requirement;
- caused docs/Skill update;
- invalidated stale assumption;
- avoided duplicate work;
- reduced time-to-evidence.

The architecture should not treat retrieval count alone as success.

## Relationship to Skills

Skills are procedural guidance. They can instruct agents to use Memory tools, ask for disconfirming evidence, or produce a dossier, but they do not own external state.

Examples:

- Reviewer Skill says: request contradicting evidence before approving.
- Orchestrator Skill says: create a Ticket shoebox before routing a risky Ticket.
- Coder Skill says: cite relevant prior decisions in the final dossier.

The Memory service provides the data and artifacts those Skills ask for.

## Relationship to Tickets/Objectives

Tickets and Objectives remain the primary durable work-management records.

- Ticket body/thread/artifacts define work authority and evidence for implementation.
- Objective item/resources define long-running design context.
- Memory can point to them, synthesize across them, and propose updates, but does not replace them.

A practical rule: if a statement changes what should be built or reviewed, it should appear in Ticket/Objective/docs, not only in Memory.

## Initial implementation slices

Do not start by redesigning every Memory record. Split into small Tickets after this architecture is accepted.

Recommended order:

1. **Task-bound shoebox artifact prototype**
   - Given a Ticket or Objective and a question, collect source refs and short rationales.
   - Store as Ticket artifact or Objective resource.

2. **Evidence file schema prototype**
   - Extract snippets from shoebox refs with source/provenance/support-refute metadata.
   - Keep as artifact/resource first.

3. **Contradiction/staleness markers**
   - Mark existing Memory decisions or source refs as stale/superseded/contradicted.
   - Avoid deleting first; make staleness visible.

4. **Reviewer/Orchestrator Skill integration**
   - Update Skills to ask for shoebox/evidence before risky review/routing.
   - This tests whether the architecture changes product quality.

5. **Workspace Memory API sketch**
   - Add read-only catalog/search and shoebox/evidence endpoints once artifact prototypes stabilize.

6. **Product-impact metrics**
   - Record when evidence is cited in Ticket/review/docs and when it changes outcomes.

## Non-goals for the first redesign

- Recreating Knowledge under another name.
- Making Memory the canonical source for Ticket requirements or design decisions.
- Automatically rewriting docs/Skills/Tickets from extracted memory without explicit review.
- Building a general vector database before task-bound evidence flows are proven.
- Optimizing resident prompt stuffing before explicit shoebox/evidence usefulness is validated.
- Creating a hidden context channel that bypasses Worker history.

## Open decisions

- Whether shoebox/evidence should start as Ticket artifacts, Objective resources, or a new Workspace Memory record type.
- How much session-log search belongs in the first slice.
- Whether source refs need a shared URI scheme across Ticket/Objectives/docs/git/session/code.
- Whether Memory tools live in `worker` feature space first or are exposed only via Workspace backend APIs.
- How to represent confidence/staleness without encouraging false precision.
- What exact product-impact events are worth recording in the first implementation.

## Exit criteria for this architecture phase

This architecture is ready to split into Tickets when:

- the core flow `source -> shoebox -> evidence -> representation/hypothesis -> product` is accepted;
- storage boundaries between Memory, Ticket, Objective, docs, and Skills are accepted;
- the first prototype slice is chosen;
- non-goals are accepted so implementation does not re-create Knowledge or Workflow tracking.
