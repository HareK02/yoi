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
