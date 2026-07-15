# Generated memory records

Yoi keeps generated memory under `.yoi/memory/` for durable, low-volume context:

- `summary.md` is optional resident context.
- `decisions/*.md` capture durable decisions and rationale.
- `requests/*.md` capture durable user preferences or standing requests.

Memory records are not workspace record authority for exact implementation state. Use tickets, objectives, repository files, git history, and session logs for exact current facts.

## Historical Knowledge records

Older workspaces may contain `.yoi/knowledge/`. Knowledge is no longer an active supported feature or workspace record authority. Current memory tooling ignores it; archive or inspect those files manually if needed.

Future Agent Skills are intentionally separate and are not implemented by this design note.
