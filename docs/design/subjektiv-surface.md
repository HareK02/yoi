# Subject Memory surface generation

The subject Memory surface is a bounded start-time context derived from confirmed, revisioned Memory. It is not another Memory authority and is never used as extraction or consolidation input.

## Fixed policy

The Backend captures one generation from current `active` revisions for exactly one subject. Staging records, Session history, historical revisions, and prior surfaces are not queried.

Selection is deterministic:

1. categories use the fixed order `preference`, `constraint`, `decision`, `working_assumption`, `open_question`, `lesson`;
2. each category is ordered by `updated_at DESC, memory_id ASC`;
3. at most 8 records are retained per category;
4. records are allocated round-robin by category, up to 24 records total;
5. the 10,000-token material budget is initially divided equally across categories as a fairness reservation; unused shares are then reclaimed in the same canonical round-robin order, so one large category cannot exclude another non-empty category or strand usable capacity;
6. both category and aggregate material bounds use the provider-independent estimate `ceil(UTF-8 bytes / 4)`;
7. before invoking the editor, the Worker measures the actual normalized Agen request fields containing the rendered system prompt, conversation items (including the fixed question and material JSON), and the typed `SubmitMemorySurface` tool definition/schema against the 12,000-token complete-input budget. It reapplies equal per-category fairness reservations under that final estimator, reclaims unused shares in canonical order, fails rather than publishing a false empty surface if no grounded material fits, and guards every subsequent correction request before it reaches the provider. Provider-specific HTTP envelope bytes are outside this normalized request budget.

The clean-context editor uses the effective `builtin:subjektiv-memory-consolidation` profile model (`codex-oauth/gpt-5.6-luna`, medium reasoning). A generation permits three model turns (the initial attempt plus at most two correction turns). A store-generation conflict causes at most one complete re-read and regeneration. The published Markdown is capped at 1,024 tokens by the same estimate.

Every editor point carries one or more exact `{memory_id, revision}` references. The Host checks the output shape, non-empty grounding, budget, subject scope, membership in the captured materials, and current store generation. These checks prove reference existence and freshness only; they do not claim semantic correctness.

Publication is idempotent for identical output at one store revision. A different concurrent output cannot replace the first published surface. Confirmed Memory writes atomically mark the prior surface stale. Failed surface work never rolls back confirmed Memory or candidate disposition.

Resident injection loads only a `ready` snapshot built from the subject's exact current store revision. The resident Backend contract transports typed `ready`, `un-generated`, `failed`, and `stale` availability separately from optional Markdown, so a valid ready-empty snapshot cannot be conflated with omission or request failure. `un-generated`, `failed`, and `stale` states are omitted rather than represented as "no Memory". A successful empty generation is represented by a current ready snapshot with empty Markdown. Snapshots created before this generation policy existed are retained as immutable history but migrate as `stale` and must be regenerated before injection. Runtime-owned new Workers install Features before durably materializing the initial session head, ensuring the current contribution is present in their initial prompt. Restored Workers preserve that persisted prompt and all prior history verbatim, then append one durable restore-boundary system item that supersedes earlier resident summaries with the latest ready surface, an explicit ready-empty state, or a no-current-surface tombstone; subsequent turns in the same restored process do not append it again.

## Semantic fixture and verification

Use a test subject with these active materials:

- constraint: “Never send credentials to tools.” (`staleness`: “Until credential brokering is redesigned.”)
- constraint: “A trusted broker may deliver short-lived credentials to the Git provider.”
- decision: “Publish only the Ticket work branch; never force-push.”
- working assumption: “The provider resolves selectors after a normal push.”
- open question: “Does the provider preserve approval after target-only movement?”
- lesson: “Checking only the local branch previously produced non-reviewable evidence.”
- duplicate lesson: “Local HEAD alone is not immutable review evidence.”

A valid review fixture must:

- retain “never” and the credential-redesign expiry condition;
- present the apparently conflicting broker constraint without silently declaring one side invalid;
- preserve the target-only-movement item as an open question rather than a decision;
- combine the two review-evidence lessons only if both meanings remain;
- cite only exact refs supplied with those materials;
- distinguish mechanical citation validation from a human/model semantic review of the wording.

The repository tests exercise deterministic bounds and fair budget reclamation, subject isolation, active-current-only selection, complete normalized editor-request accounting, output/reference rejection, empty/un-generated/failed/stale states, legacy regeneration, generation races, bounded conflict retry, terminal failure recording, parallel publication, correction/retraction staleness, and publication idempotency. The prompt fixture above is the manual semantic check for condition, negation, reservation, conflict, expiry, and duplicate editing behavior.
