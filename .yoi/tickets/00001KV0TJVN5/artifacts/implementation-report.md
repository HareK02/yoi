Implementation report for Ticket 00001KV0TJVN5

Files changed:
- `tests/e2e/src/lib.rs`
  - Added a cached e2e binary provider using `OnceLock`.
  - Preserves `YOI_E2E_BIN=<path>` as the explicit override and skips the default cargo build provider in that path.
  - Default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root, then returns the direct `target/{profile}/yoi` binary path for PTY spawning.
  - Writes `target/e2e-artifacts/binary-provider.json` and emits diagnostics with provider, build command, and binary path.
  - Expanded command-failure diagnostics to include command args.
- `tests/e2e/tests/panel.rs`
  - Updated panel tests to use the fallible cached binary provider.

Validation:
- `cargo fmt --check` — passed.
- `git diff --check` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `unset YOI_E2E_BIN && cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; default provider built the current `yoi` binary and PTY-spawned `target/debug/yoi`.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-binary-provider/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; override provider path used without invoking the default cargo-build provider.

Remaining gaps:
- None known.
