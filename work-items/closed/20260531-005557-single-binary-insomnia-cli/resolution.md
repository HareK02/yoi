Completed the umbrella migration from the previous two-command installed layout toward a single primary `insomnia` executable.

Completed phases:
- `insomnia-pod-subcommand-runtime`: moved Pod runtime startup behind `pod::entrypoint` and added `insomnia pod ...` dispatch.
- `spawn-through-insomnia-pod-subcommand`: changed internal spawn/restore defaults to typed runtime command resolution using current executable plus the `pod` prefix argument.
- `remove-insomnia-pod-binary`: removed the long-term `insomnia-pod` binary/package/devshell/flake output.
- Follow-up cleanup removed `INSOMNIA_POD_COMMAND` and `INSOMNIA_RESOURCE_DIR`, keeping the runtime command/config surface narrower.

Outcome:
- The installed package exposes `bin/insomnia` only.
- `insomnia pod ...` is the Pod runtime entrypoint; Pods remain separate processes.
- Internal spawn/restore uses typed command construction rather than shell string parsing.
- `insomnia-pod` is not kept as a compatibility alias.
- Headless `insomnia memory lint` behavior remains part of the `insomnia` CLI surface.

Validation/evidence across completed phases included:
- `insomnia pod --help`
- focused parser/spawn/restore tests
- `cargo fmt --check`
- `cargo check -p tui -p pod -p client`
- `nix build .#insomnia`
- checks that `bin/insomnia-pod` is absent
- `./tickets.sh doctor`
- `git diff --check`

Follow-up intentionally remains separate:
- `insomnia-crate-cli-owner` now tracks the architectural cleanup where the `insomnia` crate owns the product CLI/binary entrypoint and `tui` becomes a library implementation crate. That is not required for this umbrella's original single-installed-binary migration to be complete.
