# job

A shared data contract for immutable Job requests and structured results across Hosts. Validates inputs, Profile selectors, execution limits, and digests. Each Host adapter owns execution, grants, persistence, state transitions, and cleanup.

## Reference index

- [`JobRequest`] — request identity and input snapshot.
- [`JobLimits`] — execution limits that Hosts can further restrict.
- [`JobResultSubmission`] — results bound to a Job, attempt, and input.
- [`JobOutcome`] — read projection separating Job state from attempt state.
