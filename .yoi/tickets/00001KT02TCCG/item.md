---
title: "Memory prompt: conditional guidance and proactive lookup"
state: "closed"
created_at: "2026-05-31T22:35:06Z"
updated_at: "2026-05-31T22:52:35Z"
---

## Background

The normal tool-usage prompt currently contains memory guidance unconditionally:

- use targeted lookup for past decisions, prior requests, durable preferences, project history, or rationale;
- use `MemoryQuery`, `KnowledgeQuery`, and `MemoryRead` appropriately;
- treat resident memory/knowledge as helpful but stale;
- avoid querying memory every turn;
- avoid mutation unless explicitly requested or running a memory maintenance worker.

This is directionally correct, but two prompt-level issues remain:

1. Memory/Knowledge guidance should be rendered only when the corresponding memory capability is enabled and available. If memory is disabled for a Pod/profile/internal worker, the prompt should not advertise tools or behavior that cannot be used.
2. The read-side guidance is too passive. Agents should be encouraged to use small targeted queries more often when prior project decisions, user preferences, historical rationale, recent ticket work, or policy/workflow context may affect the answer or action.

This should be implemented as prompt/template behavior, not by weakening tool permissions or making memory lookup automatic.

## Requirements

- Gate memory/knowledge prompt text through the prompt/template engine based on whether memory/knowledge tools or resident memory features are enabled for the current worker.
- Do not show instructions for unavailable memory tools.
- Keep internal/disposable worker behavior conservative; do not accidentally inject normal Pod memory guidance into memory extraction/consolidation workers or other internal workers that should not see it.
- Strengthen normal Pod read-side guidance so agents perform small targeted `MemoryQuery` / `KnowledgeQuery` lookups more proactively when history may matter.
- Preserve the existing warnings:
  - memory may be stale;
  - authoritative current state is in user instructions, repository files, tickets, git history, and session logs;
  - do not query memory mechanically every turn;
  - mutation tools are for explicit user requests or memory maintenance workers.
- Do not introduce automatic pre-request memory queries in this ticket.
- Do not change memory extraction/consolidation semantics except as needed to keep prompt rendering coherent.

## Suggested prompt direction

The final wording does not need to match this exactly, but should convey:

```text
Use memory and knowledge proactively when the request may depend on prior
project decisions, historical rationale, durable user preferences, recently
completed tickets, or established workflow/policy conventions. Prefer a small
targeted MemoryQuery/KnowledgeQuery before relying on vague recollection.

Strong lookup triggers include: the user says "recently", "previously", "that
decision", "the ticket", "why", "policy", or "workflow"; you are about to make
a design recommendation; you are reviewing, merging, closing, or rescoping a
work item; or you are about to assert project history from memory.

Do not query memory mechanically on every turn. Skip memory lookup for purely
local facts answered by current repository files, command output, or current
user instructions.
```

## Acceptance criteria

- Normal Pod prompts include memory/knowledge guidance only when the corresponding capability is enabled and available.
- Prompts for memory-disabled profiles/workers do not advertise `MemoryQuery`, `KnowledgeQuery`, `MemoryRead`, or memory mutation tools.
- Normal Pod memory guidance explicitly encourages somewhat more frequent small targeted queries when history, rationale, preferences, tickets, policy, workflow, or prior decisions may matter.
- Existing stale/authority cautions and no-mechanical-query guidance remain.
- Internal memory extraction/consolidation prompts remain focused on their worker roles and do not inherit normal Pod guidance accidentally.
- Tests or focused snapshots cover enabled vs disabled prompt rendering, or the implementation otherwise includes a clear focused regression test for conditional rendering.
- `cargo fmt --check`, relevant prompt/worker tests, `./tickets.sh doctor`, and `git diff --check` pass.
