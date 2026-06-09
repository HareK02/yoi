Removed the old `insomnia-pod` installed/runtime alias and completed the next single-binary CLI phase.

Implementation:
- Removed the `insomnia-pod` binary target from the `pod` crate and made it library-only with `autobins = false`.
- Deleted the old `crates/pod/src/main.rs` wrapper while keeping `pod::entrypoint` for `insomnia pod ...`.
- Updated runtime command resolution to default to the current `insomnia` executable plus the `pod` prefix argument, while preserving executable-only `INSOMNIA_POD_COMMAND` override behavior.
- Updated Nix packaging so `.#insomnia` installs `bin/insomnia` and asserts that `bin/insomnia-pod` is absent.
- Removed `apps.insomnia-pod` from `flake.nix` and removed the devshell `insomnia-pod` wrapper.
- Updated current docs to use `insomnia pod ...` instead of the old user-facing `insomnia-pod ...` command.

Review:
- External reviewer `remove-insomnia-pod-reviewer-20260531` approved implementation commit `0d7d3c7bf1e1becb85ca8322e4b72730a91e3513`.
- Review was recorded in `thread.md` before merge.

Validation on develop after merge:
- `cargo fmt --check`
- `cargo test -p tui parse_pod_subcommand_uses_runtime_mode`
- `cargo test -p pod-command`
- `cargo test -p pod --lib discovery::tests`
- `cargo test -p pod --test spawn_pod_test`
- `cargo check -p tui -p pod -p client`
- `nix build .#insomnia`
- `./result/bin/insomnia pod --help`
- `test ! -e ./result/bin/insomnia-pod`
- `./tickets.sh doctor`
- `git diff --check`
