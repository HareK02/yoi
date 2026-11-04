## MR records

MR bookkeeping is separate from selecting and adopting changes. `CompleteMergeRequest` records an already-applied exact-source integration; its approval requirement is not a general Git adoption gate. Partial or adapted adoption is not full MR integration.

`ShowMergeRequest` defaults to a compact `summary` computed from complete evidence, not the returned history page. For review bodies/findings or prior events, request `view: "detail"`, follow `event_page.next_after`, or use a returned event sequence with `after: sequence - 1, limit: 1`. Summary readiness is an observation, not permission to integrate; unknown source observations remain unknown.

When using MRs, address each by `merge_request_id`. Reuse the repository's open MR and its `selector_from`/`selector_to`; publish source updates with a normal non-force push. Before formal review, verify the published source matches committed HEAD. Keep review/fix evidence on the MR, not repeated Ticket comments.

Record validation evidence once on the MR or an accessible session entry, then cite it: source/tree or dirty snapshot identity, command and scope, relevant features/toolchain/environment, exit or timeout and coverage reached, log reference, and unresolved issues. Reuse evidence for unchanged code under equivalent conditions. For a new candidate, assess dependencies, generated code, lockfiles, features, harness/shared initialization, migrations, and environment—not just non-overlapping files—and rerun affected or uncertain checks. Distinguish reused evidence from tests actually executed on the new source. Classify failures as regression, reproduced on the baseline, environment-dependent, or unresolved; do not call an unclassified failure or a timeout success. Broad tests are appropriate when the impact warrants them, not automatically after every local fix.

Formal approval binds the source, Ticket `content_digest`, and linked MR/result snapshot. Changed source or requirements need fresh attestation; target-only movement preserves approval. `CheckMergeRequestReadiness` checks this recording workflow. Ticket close/work release does not require reopen/reassignment for MR processing.
