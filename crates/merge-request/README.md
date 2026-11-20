# merge-request

Handles SQLite Merge Request records, MR-local Worker ownership, review evidence,
and integration results. Ownership is independent of lifecycle and approval;
transfers fence delayed writers without invalidating retained review evidence.
Does not grant Worker/resource permissions, observe Repository sources, execute
Git integration, or decide Ticket completion.

## Main entry points

- [`MergeRequestStore`]: records and state updates
- [`MergeRequest::integration_approval`]: approval evidence for stored integration results
- [`requirement_approval`]: approval evidence for current Ticket requirements and the related source set
