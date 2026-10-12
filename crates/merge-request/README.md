# merge-request

Handles SQLite Merge Request records, review evidence, and integration results.
Does not observe Repository sources, execute Git integration, or decide Ticket completion.

## Main entry points

- [`MergeRequestStore`]: records and state updates
- [`MergeRequest::integration_approval`]: approval evidence for stored integration results
- [`requirement_approval`]: approval evidence for current Ticket requirements and the related source set
