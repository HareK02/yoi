<!-- event: create author: tickets.sh at: 2026-05-31T05:47:28Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T05:48:20Z -->

## Plan

Implementation plan:

1. Remove or demote the `insomnia-pod` Cargo binary target if active code/tests no longer require it; keep `pod` as a library crate.
2. Update Nix packaging/install checks and flake app outputs so the package exposes only `insomnia`; install checks should call `insomnia pod --help`.
3. Update devshell wrapper behavior. Since internal default spawning now uses `current_exe() + ["pod"]`, the old `insomnia-pod` wrapper should be removed unless a focused dev reason remains.
4. Update current user docs (`docs/nix.md`, `docs/pod-factory.md`, manifest/profile docs, architecture docs) from `insomnia-pod` to `insomnia pod`. Do not rewrite historical work-item artifacts.
5. Validate runtime parser/help, internal spawn/restore tests, Nix build, doctor, and diff check.


---

<!-- event: review author: hare at: 2026-05-31T06:08:34Z status: approve -->

## Review: approve

External reviewer: `remove-insomnia-pod-reviewer-20260531`
Reviewed implementation commit: `0d7d3c7bf1e1becb85ca8322e4b72730a91e3513` (`cli: remove insomnia-pod binary output`)
Verdict: approve

Summary:
- The implementation removes the old `insomnia-pod` installed/runtime alias and keeps Pod runtime process separation via `insomnia pod ...`.
- `pod` is now library-only (`autobins = false`), `pod::entrypoint` remains available, and the old `crates/pod/src/main.rs` binary target is gone.
- Nix/package/devshell/docs were updated so the primary installed command is `insomnia`, `apps.insomnia-pod` is no longer advertised, and install checks use `insomnia pod --help` plus a negative check for `bin/insomnia-pod`.
- Internal spawn/restore paths use `PodRuntimeCommand::resolve()` and `prefix_args()` with default `current_exe() + ["pod"]`; `INSOMNIA_POD_COMMAND` remains executable-only.

Requirements mapping:
- Acceptance criteria are satisfied: installed output no longer exposes `bin/insomnia-pod`; `flake.nix` no longer advertises `apps.insomnia-pod`; `insomnia pod --help` remains the runtime entrypoint; relevant tests and docs were updated.
- No hidden non-goal changes were found: no `tui` crate rename, no controller/runtime process merge, and no Pod protocol or runtime flag semantic change.

Blockers: none.

Non-blocking follow-ups:
- A test name in `crates/pod/src/entrypoint.rs` still contains old-ish wording (`subcommand_help_uses_insomnia_pod_invocation`) even though the test contents use `insomnia pod`; renaming it later would improve readability.
- `docs/plan/network-peering.md` still has generic wording around a "pod binary"; this is not a user-facing `insomnia-pod` command example and is not blocking.

Validation adequacy:
- Coder-reported validation covers fmt, parser/unit/integration tests, `cargo check`, Nix build/install smoke, `nix flake check`, ticket doctor, and diff whitespace check.
- Reviewer performed read-only static review, clean status/diff inspection, residual `insomnia-pod` search, and `git diff --check`.


---
