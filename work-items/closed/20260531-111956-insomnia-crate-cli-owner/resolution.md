Moved product CLI/binary ownership from `tui` to `insomnia`.

Implementation:
- Moved `PodRuntimeCommand` from the transitional `insomnia` helper role into `client`, so lower crates no longer depend on the product CLI crate.
- Made `tui` a library implementation crate and exposed launch APIs for already-parsed modes/options.
- Added the `insomnia` package binary entrypoint and moved top-level CLI parsing/dispatch there.
- Routed `insomnia pod ...` through `pod::entrypoint` from the `insomnia` crate.
- Routed `insomnia memory lint ...` from the `insomnia` crate.
- Kept normal TUI launch behavior for resume, multi-Pod dashboard, Pod name, `--pod`, and `--session` modes.
- Updated packaging so `package.nix` builds package `insomnia` and still exposes only `bin/insomnia`.
- Updated active development/docs references away from `cargo run -p tui -- ...`.

Review:
- External reviewer `insomnia-cli-owner-reviewer-20260531` initially requested changes for a parser regression around `--resume` combined with Pod selection.
- Fix commit `37281b6` restored mutual exclusion for `-r --pod`, `--pod -r`, and `-r <podname>`, and improved `--multi --pod` diagnostics.
- Reviewer approved after re-review.

Validation after merge:
- `cargo fmt --check`
- `cargo check -p client -p pod -p tui -p insomnia`
- `cargo test -p client`
- `cargo test -p pod`
- `cargo test -p tui`
- `cargo test -p insomnia`
- `cargo run -p insomnia -- --help`
- `cargo run -p insomnia -- pod --help`
- `cargo run -p insomnia -- memory lint --help`
- invalid/conflict CLI smoke tests for `--session not-a-uuid`, `-r --pod`, `--pod -r`, and `-r <podname>`
- dependency checks confirming `client`, `pod`, and `tui` do not depend on `insomnia`, and `tui` does not depend on `pod`
- `nix build .#insomnia`
- `test -x ./result/bin/insomnia`
- `test ! -e ./result/bin/insomnia-pod`
- `./result/bin/insomnia pod --help`
- `./tickets.sh doctor`
- `git diff --check`
- `rg "cargo run -p tui" docs README.md crates package.nix flake.nix || true` produced no active hits.

Notes:
- Validation emitted pre-existing dead-code warnings in `llm-worker`/`tui` but passed.
