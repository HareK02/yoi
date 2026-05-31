<!-- event: create author: tickets.sh at: 2026-05-31T04:32:39Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T04:33:28Z -->

## Plan

Implementation plan:

1. Move current `crates/pod/src/main.rs` startup logic behind a `pod` crate library entrypoint, leaving the existing binary as a thin temporary wrapper.
2. Add a reserved `pod` subcommand to the `insomnia` binary that delegates to the same Pod runtime entrypoint.
3. Keep internal spawn defaults and Nix installed commands unchanged in this first step unless the change is smaller than expected; the long-term plan is still to remove `insomnia-pod` after internal callers use `insomnia pod`.
4. Add/update parser tests so `insomnia pod` is reserved and `insomnia --pod pod` remains available for a Pod literally named `pod`.
5. Validate focused CLI/parser/runtime help behavior and affected crates.


---

<!-- event: implementation_report author: hare at: 2026-05-31T04:46:52Z -->

## Implementation report

Implementation report from coder Pod `insomnia-pod-subcommand-coder-20260531`:

- Branch: `insomnia-pod-subcommand-runtime`
- Commit: `544257b` (`cli: add insomnia pod runtime entrypoint`)
- Extracted current Pod runtime startup into `pod::entrypoint` with `run_cli()` / `run_cli_from(...)` sharing one `run_cli_inner(...)` path.
- Kept existing `insomnia-pod` binary as a thin temporary wrapper around the shared entrypoint.
- Added reserved `insomnia pod ...` dispatch in the user-facing `insomnia` binary.
- `insomnia pod --help` reaches the Pod runtime parser with `Usage: insomnia pod ...`.
- `insomnia memory lint` remains a headless path before TUI/raw-terminal/Pod side effects.
- Pod named `pod` remains addressable with explicit `--pod pod`.
- Internal spawn default migration and Nix removal of `insomnia-pod` were intentionally left for follow-up phases.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p tui parse_` passed
- `cargo test -p pod subcommand_help_uses_insomnia_pod_invocation` passed
- `cargo run -q -p tui -- pod --help` + grep passed
- `cargo run -q -p pod --bin insomnia-pod -- --help` + grep passed
- `cargo run -q -p tui -- memory lint` passed
- `cargo check -p tui -p pod -p client` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed

Unresolved follow-ups:

- Switch internal spawn/restore defaults to `insomnia pod` via typed `program + prefix_args` command resolution.
- Remove `insomnia-pod` from packaging once internal callers no longer depend on it.


---

<!-- event: review author: hare at: 2026-05-31T04:46:53Z status: approve -->

## Review: approve

External review by reviewer Pod `insomnia-pod-subcommand-reviewer-20260531`: approve.

Reviewer summary:

- The existing `insomnia-pod` runtime startup is now a library-callable `pod::entrypoint` path.
- Both the temporary `insomnia-pod` wrapper and the new `insomnia pod ...` path call the same runtime entrypoint.
- `insomnia pod --help` reaches the Pod runtime parser/help with the appropriate invocation name.
- Existing Pod runtime flags, semantics, and `INSOMNIA-READY` stderr handshake are preserved.
- `insomnia memory lint` remains headless and returns before terminal/TUI setup.
- Existing CLI behavior is preserved except that leading positional `pod` is now a reserved subcommand; explicit `--pod pod` remains available.
- Spawn/Restore default migration and Nix removal were correctly left for later phases.

Blockers: none.

Non-blocking follow-ups:

- Migrate `client::spawn_pod`, `SpawnPod`, and restore/discovery paths to typed `program + prefix_args` so default runtime startup can become `insomnia pod ...`.
- Add installed-binary smoke coverage once CLI E2E exists.


---

<!-- event: implementation_report author: hare at: 2026-05-31T04:50:13Z -->

## Implementation report

Main workspace validation after merge:

- `cargo fmt --check` passed
- `cargo test -p tui parse_` passed
- `cargo test -p pod subcommand_help_uses_insomnia_pod_invocation` passed
- `cargo run -q -p tui -- pod --help` smoke passed (`Usage: insomnia pod`, Pod runtime flags present)
- `cargo run -q -p pod --bin insomnia-pod -- --help` smoke passed (`Usage: insomnia-pod`, Pod runtime flags present)
- `cargo run -q -p tui -- memory lint` reached the headless command path. It exited `1` because the current generated `.insomnia/memory` contains existing lint errors, which is the expected lint-failure exit code; no TUI/Pod startup side effects were observed.
- `cargo check -p tui -p pod -p client` passed with pre-existing dead-code warnings
- `./tickets.sh doctor` passed
- `git diff --check` passed


---

<!-- event: close author: hare at: 2026-05-31T04:50:14Z status: closed -->

## Closed

Added the first single-binary migration step: Pod runtime startup is now available as a shared `pod::entrypoint` library path, the existing `insomnia-pod` binary is a temporary thin wrapper, and the user-facing `insomnia` binary now supports `insomnia pod ...` with the same Pod runtime parser/help. Existing internal spawn defaults and Nix packaging are intentionally unchanged for this step. External review approved and validation passed.


---
