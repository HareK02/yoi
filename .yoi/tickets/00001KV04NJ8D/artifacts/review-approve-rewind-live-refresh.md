## Review: approve

Reviewed implementation commits `949ceb5a` and `3a7edbde` against Ticket `00001KV04NJ8D` intent and acceptance criteria.

Evidence:
- `Event::RewindApplied` no longer gates transcript restoration on `App::greeting`; it clears/replays the Pod-provided post-rewind entries through a shared restore path and emits an explicit warning if greeting metadata is unavailable, avoiding silent stale-view success.
- Rewind picker submit now enters an `applying` state, suppresses repeated `Enter`/navigation, shows an applying header, blocks `Esc` from hiding an in-flight destructive request, and closes on successful `RewindApplied`.
- Rewind failure (`Event::Error` while applying) clears the pending state, leaves the existing transcript intact, and surfaces an actionbar-visible failure plus normal error block.
- A short rewind refresh fence drops display-mutating stale live events after successful restore until authoritative `Status`/`Snapshot`; no Pod protocol or persistence semantics changed.
- Temporary investigation logging was not present in the final diff.
- Focused tests cover successful restore/old-tail removal, missing-greeting restore, duplicate-submit suppression with failure preservation, and stale live update suppression; existing rewind picker tests still pass.

Validation performed:
- `git diff --check 20daae0c..HEAD`: PASS.
- `cargo test -p tui rewind_refresh_tests`: PASS (4 tests).
- `cargo test -p tui single_pod::tests::rewind_picker`: PASS (2 tests).
- `cargo fmt --check`: PASS.
- `cargo check -p protocol -p pod -p tui`: PASS.
- `cargo test -p tui`: FAILED only in the already-reported unrelated tests: `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace` and `spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}`; rewind-focused tests passed in that run.

Residual note:
- The stale-update fence intentionally relies on the Pod's follow-up `Status`/`Snapshot` to clear; this matches the current `RewindApplied`/`Status` flow and is acceptable for this Ticket.

Decision: approve.
