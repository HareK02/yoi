Added `INSOMNIA_POD_RUNTIME_COMMAND` as a narrow development escape hatch for dogfooding/self-rebuild cases where `current_exe()` can point at a deleted debug binary.

Implementation:
- Added the override in `client::PodRuntimeCommand`.
- Unset/empty values preserve the default `current_exe() + ["pod"]` behavior.
- Non-empty values replace only the executable path and still automatically receive the `pod` prefix argument.
- The value is not shell-parsed and is not argument-split.
- Spawn/restore failure diagnostics include the resolved runtime command.
- Documented the variable in `docs/environment.md` as development-only, not normal user configuration.
- Did not reintroduce `INSOMNIA_POD_COMMAND` or old executable-without-prefix semantics.

Review:
- External reviewer `dev-pod-runtime-env-reviewer-20260531` approved implementation commit `0031953ed352ba7fae9e798b6aeee1e8ea080816`.
- Reviewer noted a non-blocking future improvement: display formatting for runtime commands could quote argv pieces with spaces more clearly.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p client runtime_command`
- `cargo test -p client`
- `cargo check -p client -p pod -p tui -p insomnia` (passed with existing dead-code warnings)
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n "INSOMNIA_POD_COMMAND" -- ':!work-items' || true` produced no active references.
