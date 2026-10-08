# Ticket evidence and immutable Merge Request results

Ticket lifecycle, repository integration, and current requirement attestation are separate facts. An incomplete read-side evidence summary does **not** undo a recorded `done` state or a `CompleteTicket` event.

## Evidence subjects

- An open MR uses the Repository/Runtime authority's provider-resolved current source. No Server-local checkout is assumed. Failure is a tagged `source_ref_observation: { status: "unavailable", code }`, not proof that review is stale. Ticket review status is `unknown`; `missing` includes `source_ref_unavailable`, not `review_after_rescope` merely because observation failed.
- A merged MR uses its stored MergeResult's `approved_source_ref` and `approval_event_id`. Branch deletion/movement, checkout absence, and provider outage do not trigger source observation. The summary's `current_subject_ref` is the immutable merged source and observation is `not_required`.
- Integration approval must exist, approve that exact source, predate the merge, have matching ReviewRequested evidence, and not have been revoked. `integration_evidence_error` is a bounded machine code for invalid stored evidence. `merged` alone never creates approval.

A merged MR's review status/timestamps describe **integration approval**, not a later requirement approval. Paged MR detail reads evaluate the full stored result before paging; UI must not infer approval from a merge event appearing in one page.

## Current requirements

`merge_request::requirement_approval` is shared by CompleteTicket and Ticket reads. It validates an effective Reviewer approval of the current item revision and the **exact complete linked MR/source snapshot**, including the hosting MR's own source and corresponding ReviewRequested event. There is no primary-MR fallback, comment-derived approval, or timestamp-only freshness rule. A post-integration review may attest a new revision/snapshot without replacing MergeResult or its integration approval.

`approved_current_subject` describes valid source/integration approvals. `review_after_rescope` retains its wire name but now describes exact current requirement attestation, not ordering of wall-clock timestamps. `approved_review` query filtering requires that current attestation. `stale_after_rescope` requires substantive item-edit history and a known source-approved snapshot lacking current attestation; unknown observations are only `missing_evidence`.

`complete_for_integration` means the summary has valid source approvals and current requirement evidence, including for partially or fully integrated sets. It is not a claim that open repositories have been integrated, that target integration readiness has been checked, or that Ticket completion is authorized. CompleteTicket additionally requires all linked results merged and validates explicit approval identity, lifecycle, and assignment transactionally.

Writable targets authorize deliverables, not mandatory MRs for every target. Every linked MR remains part of the exact requirement snapshot; no arbitrary latest-review or single-repository shortcut is used.

Query candidate SQL narrows Ticket metadata, not live evidence. Evidence predicates are evaluated from the same full MR projection as ShowTicket, after source observation. Bounded scans can return an empty page with `has_more` and a cursor after the last scanned candidate; callers must follow that cursor rather than treat an empty page as exhaustion. This prevents stale SQL/report heuristics or early nonmatching candidates from hiding later evidence matches.
