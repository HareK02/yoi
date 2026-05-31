<!-- event: create author: tickets.sh at: 2026-05-31T11:19:56Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T12:42:22Z -->

## Plan

Read-only investigator: `insomnia-cli-owner-investigator-20260531`
Classification: implementation-ready

Conclusion:
- Proceed with this ticket as one implementation ticket, but implement in ordered internal phases: move `PodRuntimeCommand` to `client`, make `tui` library-callable, move product CLI/binary ownership to `insomnia`, then update packaging/docs.

Current code/dependency map:
- `crates/tui` currently owns the product binary target:
  - `crates/tui/Cargo.toml` has `[[bin]] name = "insomnia" path = "src/main.rs"`.
  - `crates/tui/src/main.rs` owns top-level CLI parsing, `insomnia pod ...` dispatch, `insomnia memory lint`, and TUI launch.
- `crates/insomnia` currently has no binary and mostly owns `PodRuntimeCommand` only.
- `crates/pod` is already library-only and exposes `pod::entrypoint::run_cli_from`.
- `client`, `pod`, and tests depend on `insomnia` only for `PodRuntimeCommand`, creating the wrong dependency direction for the desired architecture.
- `package.nix` builds `-p tui` even though the installed command is `insomnia`.

Recommended architecture:
```text
insomnia -> tui
insomnia -> pod
insomnia -> client
tui      -> client
pod      -> client
client   -> no insomnia
pod      -> no insomnia
tui      -> no insomnia
```

Implementation plan:
1. Move `PodRuntimeCommand` from `insomnia` to `client`.
   - Prefer `client::runtime_command` with `pub use` from `client`.
   - Update `client`, `pod`, and tests to use `client::PodRuntimeCommand`.
   - Remove `insomnia` dependency from lower crates.
2. Make `tui` a library implementation crate.
   - Add `crates/tui/src/lib.rs`.
   - Move TUI module declarations and TUI launch code behind public library API.
   - Expose narrow API such as `LaunchOptions`, `LaunchMode`, `launch(...)`.
   - Keep terminal setup/teardown in `tui` because it owns terminal behavior.
   - Remove product CLI parsing and `pod`/`memory lint` dispatch from `tui`.
3. Make `insomnia` own product CLI/binary.
   - Add `crates/insomnia/src/main.rs`.
   - Move top-level CLI parser/dispatch from `tui` to `insomnia`.
   - Route `pod` to `pod::entrypoint::run_cli_from("insomnia pod", args)`.
   - Route `memory lint` from `insomnia`, not `tui`.
   - Route normal TUI modes to `tui::launch(...)` with a typed runtime command.
4. Update packaging/docs.
   - Remove `[[bin]] name = "insomnia"` from `tui`.
   - Make `package.nix` build `-p insomnia`.
   - Update active docs/dev commands from `cargo run -p tui -- ...` to `cargo run -p insomnia -- ...`.

Important constraints:
- Do not introduce `pod -> insomnia` or `tui -> insomnia`.
- Do not keep compatibility for `cargo run -p tui -- ...`.
- Do not reintroduce `insomnia-pod`, `INSOMNIA_POD_COMMAND`, or the separate `INSOMNIA_POD_RUNTIME_COMMAND`; that dev override is tracked by `dev-pod-runtime-command-env`.
- Do not change Pod protocol/profile/manifest semantics.
- Avoid broad TUI refactoring beyond making the crate library-callable.

Critical risks:
- `tui/src/main.rs` is large and mixes CLI, terminal lifecycle, event loop, key handling, and tests. Keep changes mechanical and avoid unrelated module cleanup.
- Parser tests should move to `insomnia`; TUI behavior tests should remain with `tui`.
- `memory_lint` is headless product CLI code and should not remain routed through TUI.
- `--help` behavior needs explicit handling because the old parser mostly ignored unknown flags.

Validation:
- `cargo fmt --check`
- `cargo check -p client -p pod -p tui -p insomnia`
- `cargo test -p client`
- `cargo test -p pod`
- `cargo test -p tui`
- `cargo test -p insomnia`
- CLI smoke:
  - `cargo run -p insomnia -- --help`
  - `cargo run -p insomnia -- pod --help`
  - `cargo run -p insomnia -- memory lint --help`
  - `cargo run -p insomnia -- --session not-a-uuid`
- Dependency ownership checks:
  - `cargo tree -p client -e no-dev --depth 1`
  - `cargo tree -p pod -e no-dev --depth 1`
  - `cargo tree -p tui -e no-dev --depth 1`
  - `cargo tree -p insomnia -e no-dev --depth 1`
- If packaging changed:
  - `nix build .#insomnia`
  - `./result/bin/insomnia pod --help`
  - `test -x ./result/bin/insomnia`
  - `test ! -e ./result/bin/insomnia-pod`
- `./tickets.sh doctor`
- `git diff --check`
- `rg "cargo run -p tui" docs README.md crates package.nix flake.nix`


---

<!-- event: review author: hare at: 2026-05-31T13:17:15Z status: approve -->

## Review: approve

External reviewer: `insomnia-cli-owner-reviewer-20260531`
Reviewed implementation commits:
- `22d974a` (`cli: move product entrypoint to insomnia`)
- `37281b6` (`cli: reject resume with pod selection`)
Verdict: approve

Summary:
- Product binary and top-level CLI dispatch moved from the `tui` package to the `insomnia` package.
- `tui` is now a library implementation crate and no longer owns `pod`/`memory lint` top-level dispatch.
- `PodRuntimeCommand` moved to `client`, removing the wrong lower-crate dependency on the product CLI crate.
- `pod` remains library-only and does not depend on `insomnia`.
- Packaging now builds package `insomnia` for the installed `bin/insomnia` output.

Initial review blocker:
- `--resume` combined with `--pod` or positional Pod name selection initially stopped erroring.

Fix:
- Commit `37281b6` restored mutual exclusion:
  - `insomnia -r --pod agent` errors.
  - `insomnia --pod agent -r` errors.
  - `insomnia -r agent` errors.
  - `insomnia --multi --pod agent` reports `--multi and --pod are mutually exclusive`.

Requirements mapping:
- `insomnia` owns binary target and top-level CLI dispatch.
- `tui`, `pod`, and `client` do not depend on `insomnia`.
- `tui` does not depend on `pod`.
- `insomnia pod ...`, `insomnia memory lint ...`, and normal TUI launch modes are preserved.
- No `insomnia-pod`, `INSOMNIA_POD_COMMAND`, or `INSOMNIA_POD_RUNTIME_COMMAND` was introduced.
- No Pod protocol/profile/manifest semantic change was found.

Blockers: none.

Validation adequacy:
- Coder ran Rust tests/checks, CLI smoke, dependency ownership checks, Nix build/install smoke, doctor, diff-check, and active `cargo run -p tui` grep.
- Reviewer reran focused parser tests and manually confirmed conflict diagnostics through the built binary after the fix.


---
