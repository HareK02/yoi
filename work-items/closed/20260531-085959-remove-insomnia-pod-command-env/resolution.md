Removed `INSOMNIA_POD_COMMAND` as an active runtime/config/test override.

Implementation:
- `PodRuntimeCommand::resolve()` now always uses the typed default: current executable plus the `pod` prefix argument.
- Removed override/env parsing and related tests from the `insomnia` helper crate.
- Updated SpawnPod tests to use typed `PodRuntimeCommand` injection instead of process-wide env mutation.
- Preserved production spawn/restore behavior, detached process handling, and ready/socket delivery behavior.
- Updated docs so no active supported surface documents the removed env override.

Review:
- External reviewer `remove-insomnia-pod-command-reviewer-20260531` approved implementation commit `c618fa6`.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p insomnia`
- `cargo test -p client -p insomnia`
- `cargo test -p pod --lib discovery::tests`
- `cargo test -p pod --test spawn_pod_test`
- `cargo check -p tui -p pod -p client -p insomnia` (passed with unrelated existing warnings)
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n "INSOMNIA_POD_COMMAND" -- ':!work-items' || true` produced no active references.
