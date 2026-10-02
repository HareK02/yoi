# Subject Memory surface generation

The subject Memory surface is a bounded start-time context derived from confirmed, revisioned Memory. It is not another Memory authority and is never used as extraction or consolidation input.

## Fixed policy

The Backend captures one generation from current `active` revisions for exactly one subject. Staging records, Session history, historical revisions, and prior surfaces are not queried.

Selection is deterministic:

1. categories use the fixed order `preference`, `constraint`, `decision`, `working_assumption`, `open_question`, `lesson`;
2. each category is ordered by `updated_at DESC, memory_id ASC`;
3. at most 8 records are retained per category;
4. records are allocated round-robin by category, up to 24 records total;
5. the serialized material payload uses the repository's provider-independent token estimate, `ceil(UTF-8 bytes / 4)`, and is capped at 10,000 estimated tokens; the complete editor-input policy budget is 12,000 tokens.

The clean-context editor uses the effective `builtin:subjektiv-memory-consolidation` profile model (`codex-oauth/gpt-5.6-luna`, medium reasoning). A generation permits three model turns (the initial attempt plus at most two correction turns). A store-generation conflict causes at most one complete re-read and regeneration. The published Markdown is capped at 1,024 tokens by the same estimate.

Every editor point carries one or more exact `{memory_id, revision}` references. The Host checks the output shape, non-empty grounding, budget, subject scope, membership in the captured materials, and current store generation. These checks prove reference existence and freshness only; they do not claim semantic correctness.

Publication is idempotent for identical output at one store revision. A different concurrent output cannot replace the first published surface. Confirmed Memory writes atomically mark the prior surface stale. Failed surface work never rolls back confirmed Memory or candidate disposition.

Resident injection loads only a `ready` snapshot built from the subject's exact current store revision. `un-generated`, `failed`, and `stale` states are omitted rather than represented as "no Memory". A successful empty generation is represented by a current ready snapshot with empty Markdown. Worker prompt materialization remains a start/restore boundary and does not rewrite recorded history.

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

The repository tests exercise deterministic bounds, subject isolation, active-current-only selection, output/reference rejection, empty/un-generated/failed/stale states, generation races, parallel publication, correction/retraction staleness, and publication idempotency. The prompt fixture above is the manual semantic check for condition, negation, reservation, conflict, expiry, and duplicate editing behavior.
