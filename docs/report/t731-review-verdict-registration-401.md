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

The finding is being fixed in a forward commit on the same source ref. Source
movement requires a fresh exact-source trusted review. The original narrative
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

## Improvement proposal

A verdict-registration rejection should expose a safe, bounded internal
operation/request-event correlation and classified auth failure (without proof,
credentials, keys, request body, or raw provider errors), so the Orchestrator can
locate the corresponding Backend record. The current bare 401 leaves the
reviewer unable to distinguish authentication failure from downstream authority
rejection. Investigate under separate authorization; do not weaken the guarded
review path or retry blindly to hide the failure.
