# Review verdict validation and single-call policy

Editorial note (2026-10-10): obsolete state terminology is summarized by purpose below; cited IDs, commits and validation results still describe the original investigation, not new executions.

During T-696's independent review, `ReviewRequested` captured MR `01a10bc8-59f4-7630-8166-d8cd71943e15`, source `9812e68b4d4021ea778a40d9ead4fe545d09e8bf`, and the then-current Ticket item reference `00001M4517HE0:5`. The Reviewer reproduced a pending-upload deletion defect and attempted a structured `request_changes` verdict. The request used finding severity `medium`; the Backend returned HTTP 422, listing the accepted values as `blocker`, `major`, `minor`, and `note`. No review ID or verdict event was returned.

The Reviewer obeyed the role prompt's instruction to call `ReviewMergeRequest` exactly once and did not retry. Its prose recommendation and artifacts are evidence of investigation, not recorded review authority. The parent preserves the finding, fixes the defect, and requests a new independent review bound to the updated source; it does not invent a verdict or bypass the rejected operation.

Potential improvements:

- Publish the finding severity vocabulary as a closed tool-schema enum, matching Backend validation, rather than relying on a free-form string.
- Clarify the single-call policy for an unambiguously rejected validation request that committed no event. Keep transport ambiguity, consumed capabilities, and source movement distinct from input validation; any retry permission must come from the authority contract, not the caller's inference.
- Return explicit committed-event/capability-consumption evidence on rejection, so a caller can distinguish an absent verdict from an ambiguous write without inspecting storage.

These are observed workflow barriers and proposals, not changes to the review authority contract or implementation requirements of T-696. The original rejected request and reproduction are preserved in the parent-observed Reviewer session and `target/review/T-696/review-submission-blocker.md`.
