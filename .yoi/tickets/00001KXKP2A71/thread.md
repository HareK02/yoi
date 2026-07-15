<!-- event: create author: "yoi ticket" at: 2026-07-15T20:04:08Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-15T20:04:47Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-15T20:04:47Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-15T20:27:02Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-15T20:27:51Z -->

## Decision

Routing decision: hold_queued_due_conflict_with_active_removal_work

Reason:
- Dashboard queue authorization was inspected, and this Ticket is concrete with no outgoing blocker relation。
- However, `TicketList(inprogress)` shows active Ticket `00001KXKJGYGD` (`Remove workflow tracking and workflow resources`) currently in implementation with Coder Pod running。
- This Knowledge removal Ticket and Workflow removal Ticket likely touch overlapping surfaces:
  - `crates/worker` prompt/context/resident injection paths。
  - `crates/protocol` / `crates/session-store` reference/system-item state paths。
  - resource cleanup under prompts/resources/docs。
  - Workflow required Knowledge dependency, which this Ticket body explicitly says is expected to disappear via Workflow removal。
- Starting this Ticket now would create avoidable merge conflicts and make reviewer acceptance boundaries ambiguous。
- Therefore this pass leaves the Ticket `queued` and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / thread / relations。
- `TicketRelationQuery(00001KXKP2A71)`: incoming `depends_on` from Skills support Ticket `00001KXKMX0QM`; no blocker for this Ticket。
- `TicketOrchestrationPlanQuery(00001KXKP2A71)`: no prior records; recorded `after 00001KXKJGYGD` and waiting note in this pass。
- `TicketList(inprogress)`: `00001KXKJGYGD` active。
- Orchestrator worktree status and worktree list: active Workflow removal worktree exists。
- Visible Pods: Workflow removal Coder Pod is running。

Next action:
- Wait for `00001KXKJGYGD` to be reviewed, merged, validated, and closed。
- Then re-route `00001KXKP2A71` before starting dependent Skills support `00001KXKMX0QM`。

Escalate if:
- Human explicitly wants Knowledge removal and Workflow removal combined in one branch/worktree, or accepts parallel conflict/review-boundary risk。

---

<!-- event: decision author: orchestrator at: 2026-07-15T21:40:21Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Previously recorded conflict reason (`00001KXKJGYGD` inprogress Workflow removal) is now resolved: Workflow removal was approved, merged, validated, closed, and its implementation worktree/branch cleaned。
- `TicketRelationQuery(00001KXKP2A71)` shows only an incoming dependency from Skills support Ticket `00001KXKMX0QM`; this Ticket has no outgoing blockers。
- `TicketOrchestrationPlanQuery(00001KXKP2A71)` contains prior `after 00001KXKJGYGD` / waiting notes, now satisfied, plus the accepted plan recorded in this pass。
- `TicketList(inprogress)` is 0 件。
- Orchestrator worktree is clean and no existing `00001KXKP2A71` implementation worktree/branch was found。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- Closed `00001KXKJGYGD` outcome and current repository/worktree state。
- `TicketList(inprogress)` and orchestration worktree status。

IntentPacket:

Intent:
- Knowledge を active supported feature / workspace record authority から削除する。
- Memory は durable summary / decisions / requests として残し、Knowledge は separate record kind として廃止する。
- Future Skills support の前に、Knowledge 固有の context/reference/tool surface を削除して概念境界を単純化する。

Binding decisions / invariants:
- `.yoi/knowledge` を active workspace record authority として扱わない。
- `KnowledgeQuery` を model-visible tool schema から削除する。
- `MemoryRead` / `MemoryWrite` / `MemoryEdit` / `MemoryDelete` の `kind = knowledge` を削除する。
- resident Knowledge injection / `ResidentKnowledgeEntry` / `model_invokation` field usage を削除する。
- TUI / protocol の `#<slug>` Knowledge reference / completion / chips / styling / system-item path を削除または unsupported にする。
- memory lint / consolidation / extraction は Knowledge record を作成・更新・検査しない。
- Current docs/prompts は Knowledge を active supported feature として説明しない。
- Historical reports may remain if explicitly historical, but current design docs/prompts must not advertise active Knowledge support。
- Skills are not a Knowledge clone; Skills support remains separate dependent work after Knowledge removal。

Requirements / acceptance criteria:
- `KnowledgeQuery` and `kind = knowledge` disappear from active model-visible tool schemas。
- Worker prompt / resident context / compaction / rehydration have no Knowledge section。
- Protocol/TUI/web no longer expose active `#<slug>` Knowledge references or completions。
- Memory lint/consolidation no longer handles Knowledge records as active output/target。
- Docs/prompts are updated to Memory / Ticket / Skill / repository-file boundaries without active Knowledge guidance。
- Existing `.yoi/knowledge` data handling is explicit: ignore / diagnostic / manual archive, without large migration code。
- Affected tests/build pass and dependent Skills support remains queued until this Ticket closes。

Implementation latitude:
- Removal can be direct deletion or fail-closed unsupported diagnostic where compatibility is necessary。
- Exact treatment of old `.yoi/knowledge` directory can be ignore/diagnostic/manual archive note; do not add a broad migration subsystem。
- If a crate/type still needs `knowledge` in historical tests/fixtures, keep only clearly non-active compatibility evidence and document it in test names/comments。
- Web/TUI UX for `#` can become no completion / plain text / reserved unsupported; choose the smallest coherent behavior。

Escalate if:
- Removing Knowledge tools breaks Memory tools in a way that requires rethinking Memory API shape。
- A persisted-session compatibility issue requires more than bounded ignore/drop/diagnostic behavior。
- Skills support must be implemented now to replace a required active Knowledge path。
- A current document appears to require Knowledge as a product feature rather than historical reference。

Validation:
- `rg "KnowledgeQuery|knowledge"` with remaining hits explained/limited to non-active historical/compatibility contexts。
- `rg "#<slug>|ResidentKnowledge|model_invokation|kind = knowledge|\.yoi/knowledge"` with active hits removed or explained。
- `git diff --check`
- `cargo test -p worker --lib --tests`
- `cargo test -p memory --lib --tests` or equivalent affected memory crate tests if present。
- `cargo test -p session-store --lib --tests` if system/reference/session state touched。
- `cargo test -p tui --lib --tests` if TUI references/completions touched。
- `cargo test -p yoi --tests` if CLI/memory commands touched。
- `cd web/workspace && deno task check && deno task test` if web generated/protocol/completion types touched。
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map / likely touch points:
- Worker memory/knowledge tools and prompt/resident context paths。
- Memory store/lint/consolidation/extraction code。
- Protocol/TUI/web reference/completion/chip surfaces for `#<slug>`。
- Session/system item/reference types if Knowledge references are persisted。
- Docs/prompts mentioning Knowledge as active support。

Critical risks / reviewer focus:
- `KnowledgeQuery` or `kind = knowledge` surviving in model-visible tool schema。
- Resident Knowledge still injected through prompt/compaction/rehydration。
- `#<slug>` still parsed as active Knowledge reference/completion。
- Consolidation prompt still asking to create/update Knowledge。
- Docs/prompts continuing to teach Knowledge as supported active feature。
- Accidentally removing Memory summary/decision/request functionality while deleting Knowledge。

---

<!-- event: state_changed author: orchestrator at: 2026-07-15T21:40:26Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Workflow removal dependency/conflict is now closed and no blockers remain. Accepting queued Knowledge removal Ticket for implementation before creating worktree or spawning role Pods.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-15T22:31:18Z -->

## Implementation report

Implementation progress report:

- Coder Pod `yoi-coder-00001KXKP2A71-knowledge-removal` completed implementation in `/home/hare/Projects/yoi/.worktree/00001KXKP2A71-remove-knowledge-support` on branch `work/00001KXKP2A71-remove-knowledge-support`。
- Implementation commit: `f786e019 feat: remove active knowledge support`。
- Orchestrator inspected worktree status, branch log, commit stats, and `git diff --check f2106407..HEAD`; worktree was clean and diff check passed。

Implementation summary:
- Removed active `KnowledgeQuery` registration/schema exposure。
- Removed `kind = knowledge` from Memory tool kind support and tool descriptions。
- Removed active Knowledge frontmatter/schema/lint/query/resident-context handling from the Memory crate。
- Removed Worker resident Knowledge injection and Knowledge completion state。
- Removed protocol/TUI/web active `#<slug>` / Knowledge reference and completion paths。
- Kept bounded persisted-session compatibility: old session `SystemItem` entries serialized as `knowledge` deserialize into `LegacyKnowledgeIgnored` and do not replay archived Knowledge text into model context。
- Updated prompts/docs to describe Memory summary/decision/request behavior and note old `.yoi/knowledge/` as ignored/manual archive material。
- Did not implement Agent Skills support。

Old `.yoi/knowledge` handling:
- `.yoi/knowledge/` is no longer classified, linted, queried, consolidated, extracted, resident-injected, or treated as workspace record authority。
- Docs say older `.yoi/knowledge/` files are ignored by current memory tooling and can be manually archived/inspected if needed。
- Persisted session Knowledge system items are ignored on restore rather than injected into model context。

Coder-reported validation passed:
- `rg "KnowledgeQuery|kind = knowledge|kind: knowledge|ResidentKnowledge|model_invokation|\\.yoi/knowledge|#<slug>" --glob '!target' --glob '!Cargo.lock'`: only remaining hit is historical ignored `.yoi/knowledge/` note in `docs/design/memory-knowledge.md`。
- `rg "knowledge" crates resources docs README.md web --glob '!target' --glob '!Cargo.lock'`: remaining intended hits are historical/ignored `.yoi/knowledge/` docs, compatibility-only `LegacyKnowledgeIgnored`, doc filenames containing `memory-knowledge`, and unrelated `acknowledge` substrings。
- `git diff --check`
- `cargo test -p memory --lib --tests`
- `cargo test -p worker --lib --tests`
- `cargo test -p session-store --lib --tests`
- `cargo test -p tui --lib --tests`
- `cargo test -p yoi --tests`
- `cd web/workspace && deno task check` (passed with pre-existing/accessibility warnings only)
- `cd web/workspace && deno task test` (`52 passed`)
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Next action:
- Route to external Reviewer Pod before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-15T22:35:41Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blockers:

1. Model-visible Ticket tool description still advertises Knowledge。
   - `crates/ticket/src/tool.rs:165` appends tool description text saying `memory.language for Memory/Knowledge`。
   - This keeps Knowledge in active LLM/tool guidance and violates tool/schema surface acceptance。

2. Current design docs still describe Knowledge as active support。
   - `docs/design/overview.md:18` says the `memory` crate owns “Knowledge records.”
   - This is active design guidance, not historical note, and contradicts docs/prompts cleanup acceptance。

Positive evidence:
- `MemoryToolKind` now accepts only summary/decision/request。
- `MemoryQuery` scans only summary/decisions/requests。
- Protocol/web completions are file-only。
- Old persisted Knowledge system items are compatibility-ignored。

Validation performed:
- Static read-only review of clean worktree commit `f786e019`。
- No tests rerun because static blockers remain。

Non-blocking cleanup:
- `crates/tui/src/input.rs:237-269` has stale comments mentioning `#<typed>`, though code only triggers `@` file completion。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-15T22:41:26Z -->

## Implementation report

Follow-up implementation progress report after request_changes:

- Coder Pod `yoi-coder-00001KXKP2A71-knowledge-removal` completed the stale Knowledge guidance fix。
- Follow-up commit: `ad666503 fix: remove stale knowledge guidance`。
- Orchestrator inspected worktree status, branch log, commit stats, and `git diff --check f2106407..HEAD`; worktree was clean and diff check passed。

Fix summary:
- Updated model-visible Ticket tool description in `crates/ticket/src/tool.rs`: replaced `Memory/Knowledge` language guidance with `Memory`。
- Updated active design overview in `docs/design/overview.md`: `memory` crate now owns generated Memory summary/decision/request records, not Knowledge records。
- Updated stale TUI comment in `crates/tui/src/input.rs`: removed `#<typed>` from file-completion chip comment。
- Removed stale “Knowledge records” wording from historical test-validity report text in `docs/report/test-validity-20260612/yoi.md`。

Coder-reported validation passed:
- Focused active guidance grep for `Memory/Knowledge|Knowledge records|#<typed>` in relevant active files: no hits。
- Required Knowledge-removal grep for `KnowledgeQuery|kind = knowledge|kind: knowledge|ResidentKnowledge|model_invokation|\\.yoi/knowledge|#<slug>`: only remaining hit is `docs/design/memory-knowledge.md:13`, an explicit historical/legacy note saying old `.yoi/knowledge/` is no longer active and current tooling ignores it。
- `git diff --check`
- `cargo check -p yoi`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

No focused behavior tests were re-run for this follow-up because changes were docs/comments/model-visible text only。

---

<!-- event: review author: reviewer at: 2026-07-15T22:44:47Z status: request_changes -->

## Review: request changes

Focused re-review result: request_changes

Fixed from prior review:
- `crates/ticket/src/tool.rs:165` now says Memory only。
- `docs/design/overview.md:18` now says summary/decision/request, not Knowledge records。
- Focused grep/read review found no `KnowledgeQuery`, no `kind=knowledge` Memory tools, no resident Knowledge collection。
- `.yoi/knowledge` remains only as historical/manual note in `docs/design/memory-knowledge.md:13`。
- Legacy Knowledge system items are ignored in `crates/session-store/src/system_item.rs:142-177`。
- No Agent Skills implementation was observed in the reviewed diff。

Remaining blocker:
- Web Console still treats `#` as an active completion sigil rather than unsupported/plain text。
- `web/workspace/src/lib/workspace/console/composer-completion.ts:4` includes `"#"`。
- `composer-completion.ts:38` matches `[:@#]`。
- `composer-completion.ts:86-87` maps `#` to `"file"`。
- `web/workspace/src/routes/w/[workspaceId]/+page.svelte:316-335,357-362` resolves/applies those completions through the worker completion endpoint。
- This leaves an active `#<typed>` completion path after Knowledge removal, contrary to acceptance item requiring active `#<slug>` Knowledge references/completions to be removed or unsupported/plain text。

Validation performed:
- Static read-only review only; no tests rerun because the blocker is visible in source。

---
