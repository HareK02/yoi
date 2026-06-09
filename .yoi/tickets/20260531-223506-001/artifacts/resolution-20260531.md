Implemented and merged conditional memory prompt guidance.

Merged commits:

- `03256db memory: gate prompt guidance`
- `3a0c8e1 memory: clean prompt helper warning`
- `bdb52b1 merge: memory prompt guidance`

Review:

- Review approved in `62d88d9 review: approve memory prompt guidance`.
- Initial review requested one warning cleanup; follow-up review approved the fix.

Summary:

- Normal prompt memory/knowledge guidance is now gated by available tool capabilities.
- Memory-disabled prompts do not advertise `MemoryQuery`, `KnowledgeQuery`, `MemoryRead`, or memory mutation tools.
- Partial-capability prompts avoid naming unavailable memory vs knowledge tools.
- Normal Pod guidance now encourages somewhat more proactive small targeted lookups when prior decisions, rationale, durable preferences, tickets, policy, workflow, or project history may matter.
- Existing cautions remain: memory can be stale; current authoritative state lives in user instructions, repository files, tickets, git history, and session logs; memory should not be queried mechanically every turn; mutation tools remain restricted.
- Internal memory extraction/consolidation prompts remain focused on their worker roles and do not inherit normal Pod guidance.

Validation after merge:

- `cargo fmt --check` — passed
- `cargo test -p pod prompt::` — passed (`53 passed; 0 failed`)
- `./tickets.sh doctor` — passed
- `git diff --check` — passed

Caveats:

- The test run still reports an existing unrelated `llm-worker` dead-code warning; the new prompt helper warning found during review was removed.
