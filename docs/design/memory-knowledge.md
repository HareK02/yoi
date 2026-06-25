# Memory and Knowledge

Yoi memory is generated context, not project authority.

The authoritative record for work is still code, git history, work item files, tickets, session logs, and explicit user instruction. Memory helps the agent retrieve durable preferences and prior rationale, but it must not replace the records that made those facts true.

## Record types

- `summary.md` is resident background context for normal Workers.
- `decisions/` stores durable decisions that are useful across turns.
- `requests/` stores durable user requests and preferences.
- `.yoi/knowledge/` stores curated Knowledge records when available.
- `_logs/` stores append-only audit observations.
- `_staging/` is generated candidate state before consolidation.

Generated `.yoi/memory` is personal/generated state in this repository. Curated workflow/Knowledge assets may be tracked separately when intended.

## What memory should not do

Memory should not duplicate authoritative project records. Do not copy ticket threads, TODO lists, implementation reports, or full docs into memory merely to make them resident.

The useful memory is the small part that changes future behavior: a policy, a rationale, a user preference, or a durable conclusion that would otherwise be hard to find.

## Lookup policy

Agents should use memory and Knowledge when the request depends on prior decisions, historical rationale, project workflow, or durable preferences.

Agents should not query memory every turn. Local repository files, current user instructions, command output, and tickets are more authoritative for exact current state.

## Audit and mutation

Memory extraction/consolidation writes append-only observations under `_logs`. No-op and idle notices belong there rather than in user-facing UI.

Memory mutation should be explicit work or part of the configured memory maintenance path. Casual edits during unrelated tasks make memory harder to trust.

## Language

Memory follows configured memory language policy. Conversation prose follows the user's language unless configured otherwise.
