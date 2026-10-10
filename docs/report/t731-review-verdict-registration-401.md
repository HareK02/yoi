# T-731 independent review verdict registration rejected

## Confirmed observation

- Ticket: T-731, item revision `00001M4G6PHJQ:0`.
- Selected MR: `01a120d6-36fb-7673-92ce-a4cd50ba0d7d`, repository `main`.
- Selectors: `work/T-731-workdir-denial-diagnostics` → `develop`.
- First reviewed source: `f9d70e2ae2c1b226fd5832dfa13102b9f1b6ac4c`.
- Trusted parent-owned Reviewer binding recorded `ReviewRequested` at
  `2026-10-09T13:25:51.374292664+00:00`, MR seq2 event
  `01a120d7-6c98-7c91-bdc0-6c61d2fe66c1`.
- Reviewer found a real normal-tool diagnostic-wrapper classification regression
  through a target-only public-API harness, after 30 focused tests, root check,
  formatting and diff checks passed.
- The review-submission tool returned
  `Merge Request API returned HTTP 401` with no diagnostic response text. It did
  not record the intended request-changes verdict. The committed session's
  output reference is `E01a120e2-ae8b-7581-82f9-f0f43565c173`; its reproduction
  output is `E01a120de-c8a2-72d3-ba9b-6a9392d19dda`.
- Parent `ShowMergeRequest` read at provider observation
  `2026-10-09T13:39:10+00:00` still showed only seq1 comment and seq2 request;
  there was no verdict/approval event. The tool response does not provide an
  exact failure timestamp, auth reason, or request correlation ID. None is
  inferred here.

## Handling and limitation

The first finding was fixed in forward commit `10c55874` on the same source ref.
Source movement requires a fresh exact-source trusted review. The original narrative
verdict is not substituted for a recorded authoritative review event.

Ticket seq4 records this operational blocker for Orchestrator attention. The
Orchestrator directed continuation through the ordinary fresh trusted-review
path after fix/test/publish, and stopping with bounded evidence if submission
again returns 401. No independent Runtime Reviewer, alternate mutation endpoint,
proof/auth/capability change, deployment update, or branch/MR replacement is used.

The HTTP 401 cause is unknown. It is not established to share a cause with the
reported Workdir 403, and T-731's fixture diagnostics do not resolve either live
incident. The registration failure is not repaired by the product-code changes
in this Ticket.

## Fresh review: invalid severity rejected (422)

- Reviewed source after the first forward fix:
  `10c55874e3cc4c3562f608e10c56ff1c4c2a4d3c`.
- Fresh request: MR seq3 event `01a120f4-6c90-7791-a36f-69d52e2cbd4b`,
  `2026-10-09T13:57:31.897275884+00:00`, same Ticket revision and sole MR.
- Independent review found a second operational regression: normal Write's
  bare NotFound match failed to create files for typed/nested diagnostic context.
  A public-tool harness reproduced it despite 59 focused tests passing.
- The sole executed verdict attempt returned HTTP 422, specifically invalid
  `findings[0].severity = medium`; Backend expects blocker/major/minor/note.
  Committed response: `E01a120fc-a7b7-79c3-b812-12cfd1038f75`.
- Observed UTC bracket was 14:05:37.641361776Z–14:06:36.777627982Z on
  2026-10-09, not the exact server request timestamp. Parent MR reread at
  14:11:31Z still had no events after seq3.
- Parent requested a schema-only correction on the unchanged snapshot, but the
  Reviewer role's exactly-once instruction disallowed a second verdict call.
  Its committed follow-up confirms no corrected submission executed. No other
  mutation path or Reviewer was used to bypass this restriction.

This 422 is input validation evidence, not proof that the earlier 401 has been
resolved. The narrative request-changes judgment is not a recorded verdict.
The reproduced second regression is addressed by the shared classification
source and real Write regression in this forward change on the existing branch.
The new test fails for the typed-reason case before the fix and passes all three
forms after it. That source movement still needs fresh exact-source review, not
reuse of the old review binding. The user-reported Workdir 403 remains unidentified.

## Later registration observation

A subsequent fresh binding on source
`6ac87c885e58f11cb6dde0557c9229249f2c428e` successfully recorded request-changes:
MR seq5 event `01a12114-771a-76b1-b0fb-2fde0d56e3e1` at
`2026-10-09T14:32:31.769486531+00:00`, confirmed by parent MR reread. This proves
that selected submission succeeded; it does not explain the earlier 401.
No authentication, proof, capability, Backend, or running binary changes were
made to obtain it. The remaining code finding and routine review evidence are
on that MR, not an approval or Ticket conclusion in this report.

## Improvement proposal

A verdict-registration rejection should expose a safe, bounded internal
operation/request-event correlation and classified auth failure (without proof,
credentials, keys, request body, or raw provider errors), so the Orchestrator can
locate the corresponding Backend record. The current bare 401 leaves the
reviewer unable to distinguish authentication failure from downstream authority
rejection. Investigate under separate authorization; do not weaken the guarded
review path or retry blindly to hide the failure.

Also align the tool schema's severity vocabulary with the Backend enum, rather
than advertising an unconstrained string that consumes an exactly-once verdict
attempt on a deterministic 422. Any correction/review-grant lifecycle policy
needs a separate explicit design; this Ticket does not change it.
