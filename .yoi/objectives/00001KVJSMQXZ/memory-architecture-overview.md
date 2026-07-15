---
created_at: "2026-07-15T21:33:00Z"
updated_at: "2026-07-15T21:52:00Z"
objective: "00001KVJSMQXZ"
status: "architecture-draft"
notes: "Draft architecture for redesigning Memory / Knowledge / Skills as distinct workspace resources. This is an Objective resource, not implementation authority."
---

# Memory / Knowledge / Skills architecture overview

## Position

Yoi should treat **Memory**, **Knowledge**, and **Skills** as three distinct resource classes instead of trying to make one generic record store do everything.

The practical architecture is more important than the sensemaking model. Pirolli & Card's sensemaking process remains useful background, but it should not force Yoi into shoebox/evidence/hypothesis infrastructure before the product shape is clear. The first goal is a clear workspace resource model whose outputs are human-readable and can grow over time.

Target split:

- **Memory**: short-term facts, preferences, current focus, and in-progress context. It is written with change in mind.
- **Knowledge**: long-term notes meant to be cultivated by humans/agents, cross-linked into a mesh, and readable as durable project understanding.
- **Skill**: portable, established procedural guidance in Agent Skills format, used to perform a class of work.

This replaces the earlier draft's stronger emphasis on sensemaking artifacts as first-class architecture. Sensemaking remains a usage pattern that can be supported by Memory/Knowledge/Skills, not the central storage taxonomy.

## Design goals

- Produce human-readable artifacts that can mature.
  - Temporary model summaries are not enough.
  - A useful result should be able to grow into a Knowledge note, Skill, Ticket decision, doc, or report.
- Keep volatile and durable material separate.
  - Short-term context should not pollute long-term notes.
  - Long-term notes should not be overwritten by every session extraction.
- Keep procedures separate from notes.
  - A repeated way of doing work should become a Skill, not a Knowledge note.
- Keep authority boundaries explicit.
  - Tickets define work authority.
  - Docs and Objective resources hold maintained design context.
  - Knowledge notes hold cultivated long-term understanding.
  - Skills guide execution.
  - Memory tracks changing working context and preferences.
  - Typed feature/tool surfaces own external state changes.
- Use Workspace backend as the shared authority for resource APIs where possible.
  - Workers should not develop divergent local views when `WorkspaceClient::Http` is available.

## Resource classes

### Memory

Memory is for volatile, short-to-medium-term material that helps the agent continue work without pretending to be long-term project truth.

Memory records may include:

- current focus;
- user preferences;
- working assumptions;
- recent decisions whose authority exists elsewhere;
- in-progress constraints;
- reminders to inspect a Ticket/doc/session again;
- session-derived observations;
- personal/workspace context that is expected to change.

Memory should be written as provisional:

- include when/why it was learned;
- include scope and applicability;
- allow staleness/supersession;
- avoid copying authoritative records verbatim;
- prefer pointers to Tickets/docs/Knowledge notes when possible.

Memory is useful for resident context and lightweight lookup, but it should not be optimized as a permanent note system.

#### Memory examples

```text
User preference: prefers direct commits only when explicitly requested.
Scope: this repository / current dogfooding workflow.
Source: repeated user corrections in sessions around git operations.
Staleness: revisit if user changes repo workflow.
```

```text
Current focus: web Workspace console and Workspace-backed Ticket/Skill authority.
Source: recent Tickets and Objective updates.
Expected to change after current milestone.
```

### Knowledge

Knowledge is a long-term note system. It is meant to be grown, revised, linked, split, merged, and read by humans. It should form a mesh of project understanding rather than a pile of extracted snippets.

Target Knowledge is not the old unused Knowledge feature preserved as-is. The legacy implementation can be removed first; the replacement should be designed as a proper workspace note subsystem.

Knowledge notes should:

- be Markdown-first and human-readable;
- have stable IDs/slugs;
- support bidirectional links/backlinks;
- support tags or typed relations where useful;
- preserve provenance for important claims;
- link to Tickets, Objectives, docs, commits, reports, Skills, and other Knowledge notes;
- support review/staleness/supersession;
- be maintained intentionally, not only generated automatically.

Knowledge is where long-term architecture notes, conceptual models, subsystem explanations, decision context, recurring constraints, and domain understanding should mature.

#### Knowledge examples

- `workspace-authority-model`
  - Explains why Workspace backend is authority for Tickets/Skills/Runtime views.
  - Links to Ticket backend API design, Skill support ticket, Workspace control plane Objective.
- `memory-knowledge-skill-boundary`
  - Defines boundaries among Memory, Knowledge, and Skills.
  - Links to this architecture resource and future implementation Tickets.
- `ticket-lifecycle-authority`
  - Explains Ticket state authority and transition graph rationale.
  - Links to relevant decisions and code locations.

### Skill

Skill is an established, portable workflow/procedure for a class of tasks. It should follow Agent Skills format:

```text
.yoi/skills/<skill-name>/
  SKILL.md
  scripts/
  references/
  assets/
```

A Skill is not a state machine and does not own external authority. It is prompt/resource guidance that tells the agent how to perform a task using available tools.

Skills should contain:

- when to use the Skill;
- step-by-step procedure;
- expected inputs;
- expected outputs/report shape;
- examples;
- edge cases;
- optional references/scripts/assets.

Skills are portable when they can be moved to another workspace with minimal project-specific assumptions.

#### Skill examples

- `coder-review-cycle`
  - How a Coder should implement, validate, request review, handle feedback, and produce a dossier.
- `ticket-intake`
  - How to turn ambiguous user requests into accepted Ticket requirements.
- `architecture-review`
  - How to evaluate design proposals, alternatives, and authority boundaries.

## Boundaries

### Memory vs Knowledge

Memory is provisional and change-oriented. Knowledge is maintained and growth-oriented.

Use Memory when:

- the information is short-lived;
- the information is a preference or working assumption;
- the information is useful resident context;
- the right long-term destination is not clear yet.

Use Knowledge when:

- the information should be read and revised over time;
- the information explains a durable project concept;
- multiple future tasks should link to it;
- the note benefits from backlinks and mesh structure;
- humans should be able to browse it as project understanding.

Promotion path:

```text
Memory observation -> candidate note/update -> Knowledge note / docs / Ticket decision
```

Promotion should be explicit. Not every Memory item becomes Knowledge.

### Knowledge vs Docs

Docs are maintained public/project-facing exposition. Knowledge is internal, linked, evolving understanding.

A Knowledge note may later become a doc, but the threshold is different:

- Knowledge can contain uncertainty, partial models, and links to evidence.
- Docs should present settled explanations or user/developer guidance.

### Knowledge vs Ticket decisions

Ticket decisions are authority for work item history and state. Knowledge notes synthesize across Tickets.

If a decision changes a Ticket's requirement, state, or acceptance criteria, it must be in the Ticket. Knowledge can link to it and explain the broader pattern.

### Skill vs Knowledge

Knowledge explains what is true or how the project is understood. Skill explains how to do a recurring task.

Use Skill when the artifact should instruct an agent to perform work:

- review process;
- implementation process;
- release checklist;
- architecture evaluation method.

Use Knowledge when the artifact should explain a concept:

- Workspace authority model;
- Memory architecture;
- Ticket lifecycle rationale.

### Skill vs Feature/Plugin

Skill is prompt/resource guidance. Feature/Plugin is executable authority and tool surface.

A Skill can say "create a Ticket shoebox before review." The Workspace/Memory feature provides the typed tool/API that actually creates it.

## Workspace authority

The target architecture should be Workspace-backed.

### Memory API

Workspace backend should eventually provide:

- Memory list/search/read/write/edit/delete;
- resident memory summary generation or retrieval;
- staleness/supersession markers;
- Memory candidate proposal from sessions or artifacts;
- provenance and audit events;
- preference/current-focus surfaces.

Local `.yoi/memory` can remain compatibility/offline storage during transition.

### Knowledge API

Workspace backend should provide a proper note API rather than exposing raw filesystem layout as the only interface:

- Knowledge catalog/list/search;
- note read/write/edit/delete;
- link/backlink extraction;
- relation/tag metadata;
- staleness/supersession markers;
- note diagnostics/lint;
- source/provenance refs;
- import/export from Markdown files.

A filesystem representation may still exist, likely under `.yoi/knowledge/`, but Worker/Runtime/Web/CLI should converge on the Workspace API view when available.

### Skill API

Skill support should follow the separate Skill Ticket direction:

- Workspace backend owns discovery/lint/catalog/activation.
- `.yoi/skills/<skill>/SKILL.md` is the workspace storage convention.
- Workers use Workspace API for Skill metadata/body when `WorkspaceClient::Http` is available.
- Skill references/assets are accessed through backend-resolved authority or Skill resource APIs.

## Human-readable growth path

A core requirement is that useful material can mature into human-readable form.

Typical paths:

```text
Session observation
  -> Memory record
  -> Knowledge note candidate
  -> Knowledge note with links/backlinks
  -> maintained doc or Ticket decision if it becomes authority
```

```text
Repeated successful procedure
  -> Memory observation / Ticket comment
  -> draft Skill
  -> `.yoi/skills/<skill>/SKILL.md`
  -> builtin or shared Skill if portable
```

```text
Design discussion
  -> Objective resource
  -> Knowledge note synthesis
  -> implementation Tickets
  -> docs after stabilization
```

The architecture should make these promotions explicit and reviewable.

## Sensemaking as a usage pattern

Sensemaking remains useful, but should be treated as a pattern layered on the resource model.

Pirolli & Card's flow can map to Yoi resources as:

```text
External sources -> Memory/search/shoebox artifact -> Knowledge notes / Objective resources -> Tickets/docs/Skills/products
```

But we should not make "shoebox" or "hypothesis matrix" mandatory first-class concepts until actual workflows prove they are needed.

Initial sensemaking support can be lightweight:

- task-bound collected references as Ticket/Objective artifacts;
- evidence summaries with provenance;
- explicit contradictory evidence sections in review Skills;
- Knowledge notes that synthesize recurring patterns.

## Implementation posture

The current implementation can be redesigned. Do not preserve old Memory/Knowledge shapes just because they exist.

However, avoid a large-bang rewrite. Split after this architecture is accepted.

Recommended implementation sequence:

1. **Clarify current Memory after Knowledge removal**
   - Keep short-term/resident Memory working.
   - Remove assumptions that Memory must replace Knowledge.

2. **Design target Knowledge note model**
   - Markdown note format.
   - link/backlink model.
   - provenance/staleness metadata.
   - Workspace API surface.

3. **Implement minimal Knowledge catalog/read/write**
   - Start with Markdown files and Workspace API.
   - Add lint and backlinks.

4. **Implement Skill support separately**
   - Follow Agent Skills standard and Workspace authority.
   - Do not mix Skill with Knowledge note schema.

5. **Add promotion workflows/tools**
   - Memory -> Knowledge candidate.
   - Knowledge -> docs/Ticket decision candidate.
   - repeated procedure -> Skill candidate.

6. **Add sensemaking helpers only after resource classes stabilize**
   - collected refs;
   - evidence extraction;
   - contradiction/staleness views;
   - product-impact metrics.

## Non-goals

- Treating Memory as the only long-term knowledge store.
- Treating Knowledge as generated memory with a different name.
- Treating Skill as a Workflow tracker or state machine.
- Hiding context injection outside Worker history/tool results.
- Making Knowledge notes authoritative over Tickets/docs/git history.
- Automatically rewriting Knowledge/Skills/docs without review.
- Designing a vector database before the human-readable artifact model is stable.

## Open decisions

- Whether target Knowledge uses `.yoi/knowledge/<slug>.md`, nested directories, or an index plus notes.
- Exact frontmatter for Knowledge notes.
- Link syntax and backlink extraction rules.
- How Knowledge note IDs/slugs relate to titles.
- Whether Memory remains local-first or becomes Workspace API-first in the same phase as Knowledge.
- Whether Memory/Knowledge APIs share a crate or are separate domain crates.
- What promotion UI/tool should exist for Memory -> Knowledge.
- How to distinguish personal Memory from workspace Memory.
- How much auto-generation is allowed for Knowledge drafts.

## Exit criteria for architecture phase

This architecture is ready to split into Tickets when:

- the Memory / Knowledge / Skill boundary is accepted;
- target Knowledge as long-term linked notes is accepted;
- Workspace backend authority for Memory/Knowledge/Skills is accepted;
- the first implementation slice is chosen;
- non-goals are accepted so implementation does not recreate old Workflow tracking or old unused Knowledge unchanged.
