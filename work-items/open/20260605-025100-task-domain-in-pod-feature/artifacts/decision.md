# Decision: keep built-in Task feature inside pod for now

Task is a stateful built-in feature, not a low-level `tools` crate concern. The next step should keep the feature inside `pod` rather than creating a new crate.

Rationale:

- `pod::feature` / `pod::hook` are still defined in the `pod` crate.
- Creating `builtin-features` now would either depend on `pod` or require premature extraction of a feature-api crate.
- Feature-per-crate would create too many crates; a future `builtin-features` crate may be appropriate only after the API boundary is stable.
- Moving Task domain state out of `tools` and into `pod::feature::builtin::task` fixes the immediate semantic split without forcing crate-boundary churn.

Desired result for this ticket:

- `tools` provides low-level generic tool helpers.
- `pod::feature::builtin::task` owns TaskStore, Task types, Task tool implementations, Task reminders, and Task feature lifecycle.
