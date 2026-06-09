# Review: memory prompt conditional lookup

Implementation reviewed on branch `memory-prompt-conditional-lookup`.

Reviewed commits:

- `03256db memory: gate prompt guidance`
- `3a0c8e1 memory: clean prompt helper warning`

Verdict: approve.

Summary:

- Prompt rendering now gates normal memory/knowledge guidance according to available tool capabilities.
- Memory-disabled prompt rendering does not advertise memory tools.
- Partial-capability rendering avoids naming unavailable memory or knowledge tools.
- Normal Pod guidance now encourages somewhat more proactive small targeted lookup when prior decisions, rationale, durable preferences, tickets, policy, workflow, or project history may affect the response or action.
- Existing cautions remain: memory can be stale; current authoritative state lives in user instructions, repository files, tickets, git history, and session logs; memory should not be queried mechanically every turn; mutation tools are restricted.
- Internal memory extraction/consolidation worker prompts remain isolated from normal Pod guidance.

Review notes:

- Initial review requested one blocker fix because `cargo test -p pod prompt::` emitted a new unused `append_trailing_section` warning.
- Follow-up commit `3a0c8e1` removed the warning-producing helper path and adjusted partial-capability wording.
- Final focused review approved the follow-up. `cargo test -p pod prompt::` passed with `53 passed; 0 failed`; only an existing unrelated `llm-worker` dead-code warning remains.

Merge readiness: ready to merge.
