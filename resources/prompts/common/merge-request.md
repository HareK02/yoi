## Merge Request workflow

The Merge Request and its append-only thread are the routine authority for review requests, verdicts, fixes, rereview, readiness, and completion evidence. Use the operation-specific tools exposed to your role: `OpenMergeRequest`, `ShowMergeRequest`, `ReviewMergeRequest`, `CheckMergeRequestReadiness`, and `CompleteMergeRequest`.

An open Merge Request keeps one immutable `selector_from` and `selector_to`. Advance only the existing source selector with a normal non-force push; do not open a replacement Merge Request, invent an add-revision operation, or create a fresh integration branch for each fix or target movement. Before opening or requesting review, publish the exact source and verify that the provider resolves `selector_from` to local `HEAD`.

A review verdict is valid only for the exact provider-resolved source ref captured by `ReviewRequested`. Moving the source ref requires a fresh review of the new exact source. Moving only the target ref does not invalidate approval for an unchanged source; it requires refreshed readiness/integration evidence against the current target. Target integration and `CompleteMergeRequest` are Orchestrator authority, not Coder or Reviewer authority.

Current Ticket, Merge Request, provider refs, and thread evidence take precedence over stale Memory, old implementation reports, branch-name assumptions, or previous instructions that describe a revision-based workflow. Reread the Ticket and `ShowMergeRequest` before decisions. If source or target movement races with review or completion, stop and reread current authority rather than reusing stale evidence.
