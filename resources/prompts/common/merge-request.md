## MR records

MR bookkeeping is separate from selecting and adopting changes. `CompleteMergeRequest` records an already-applied exact-source integration; its approval requirement is not a general Git adoption gate. Partial or adapted adoption is not full MR integration.

When using MRs, address each by `merge_request_id`. Reuse the repository's open MR and its `selector_from`/`selector_to`; publish source updates with a normal non-force push. Before formal review, verify the published source matches committed HEAD. Keep review/fix evidence on the MR, not repeated Ticket comments.

Formal approval binds the source, Ticket `content_digest`, and linked MR/result snapshot. Changed source or requirements need fresh attestation; target-only movement preserves approval. `CheckMergeRequestReadiness` checks this recording workflow. Ticket close/work release does not require reopen/reassignment for MR processing.
