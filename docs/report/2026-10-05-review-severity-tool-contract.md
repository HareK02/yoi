# Review finding severity contract mismatch during T-697

The independent Reviewer completed source/requirement analysis and validation,
then its one `ReviewMergeRequest` submission was rejected with HTTP 422 because
it used severity `medium`. The tool exposed a free-form string, while Backend
validation accepts only `blocker`, `major`, `minor`, or `note`. No verdict was
recorded by that failed call. The child preserved the source and reported the
blocked submission rather than loop-retrying a mutation.

The parent inspected the committed review session and fresh MR authority,
confirmed the same request/source/Ticket snapshot with no verdict, and submitted
a fresh child turn to correct the input contract. The Reviewer independently
selected supported severities and recorded `request_changes` successfully on
the same MR. No alternate token/context, CLI/storage write, source movement or
replacement MR was used to compensate for the rejection.

Suggested improvement: expose finding severity as the typed Backend enum in the
LLM tool schema, and document allowed values in the Reviewer prompt. Rejecting
an unadvertised enum after a costly review should not prevent durable findings
from reaching the MR. This report is tool feedback, not a T-697 product
constraint or a request to alter review judgments.
