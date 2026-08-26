# Merge Request open requires an undiscoverable Ticket repository target

## Observed

While implementing Ticket `00001KZRNHB35`, the assigned Coder had a clean committed Workdir and all immutable base/head/changed-path evidence needed by `OpenMergeRequest`.

`OpenMergeRequest` rejected candidate repository ids with:

```text
invalid input: Merge Request repository must match the authoritative Ticket target
```

The typed `TicketShow` result available to the Coder rendered only the Ticket id and state; it did not expose the authoritative `repository_id` or ref selector. No typed repository/workdir lookup tool was available to the Coder. Continuing would therefore require guessing control-plane identity, mutating the Ticket target without evidence, or bypassing typed authority, all of which are correctly prohibited.

## Impact

A Coder can finish and validate implementation but cannot open the required selector-based Merge Request or start independent review. The failure is safe, but it strands otherwise review-ready work and provides no actionable expected target.

## Suggested improvement

At least one trusted read surface in the assigned-Coder flow should return the immutable Ticket target needed by `OpenMergeRequest`:

- include `repository_id` and `ref_selector` in `TicketShow`'s bounded authoritative projection; or
- have `OpenMergeRequest` derive repository identity from the authoritative Ticket target and remove it from model input; or
- return a bounded structured mismatch diagnostic containing the authoritative repository id when the caller is already authorized to read that Ticket.

Deriving the repository in `OpenMergeRequest` is preferable because it removes duplicated model-controlled identity and avoids target drift between Ticket read and MR creation.
