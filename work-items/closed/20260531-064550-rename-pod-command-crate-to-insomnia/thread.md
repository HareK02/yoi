<!-- event: create author: tickets.sh at: 2026-05-31T06:45:50Z -->

## Created

Created by tickets.sh create.

---

<!-- event: close author: hare at: 2026-05-31T06:49:44Z status: closed -->

## Closed

Renamed the runtime command helper crate/package from `pod-command` to `insomnia` to align the shared helper crate with the installed binary name.

Implementation:
- Renamed `crates/pod-command` to `crates/insomnia`.
- Changed package name from `pod-command` to `insomnia`.
- Updated workspace membership and workspace dependencies.
- Updated `client` and `pod` crate dependencies/imports from `pod_command` to `insomnia`.
- Kept `PodRuntimeCommand` behavior unchanged: default runtime command remains current executable plus `pod`, and `INSOMNIA_POD_COMMAND` remains executable-only.
- Updated test helper names to avoid stale `pod_command` wording.

Validation:
- `cargo fmt --check`
- `cargo test -p insomnia`
- `cargo test -p client -p insomnia`
- `cargo test -p pod --lib discovery::tests`
- `cargo test -p pod --test spawn_pod_test`
- `cargo check -p tui -p pod -p client -p insomnia`
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n "pod-command\|pod_command" -- ':!work-items' ':!docs' ':!Cargo.lock' || true`


---
