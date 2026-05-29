<!-- event: create author: tickets.sh at: 2026-05-29T18:33:18Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: insomnia at: 2026-05-29T19:37:45Z status: approve -->

## Review: approve

Reviewed merged implementation from branch `work/builtin-profile-remove-manifest-cascade` (`625730c`, follow-up `20ac0c9`, merged as `merge: builtin profile default startup`).

Approved after blocking fixes:

- `insomnia-pod --overlay` is no longer accepted as a user-facing generic TOML layer; SpawnPod now uses hidden typed `--spawn-config-json` and TUI restore uses typed `--session-pod-name` / `--resume-scope-json`.
- `insomnia-pod --pod <name>` fresh-create compatibility is explicit: absent Pod metadata resolves the default profile and applies the requested pod name as a typed override, with test coverage.
- TUI fresh spawn no longer has `cascade_has_scope`, `CwdDefault`, or cwd write-scope injection. Scope comes from the selected profile; session restore passes only the persisted scope snapshot.
- `resources/nix/profiles/default.nix` matches the current dogfooding manifest values and builtin profile resolution resolves `target = "."` against the launch workspace/current directory rather than the bundled profile directory.
- Profile and one-file manifest paths share `PodManifestConfig::builtin_defaults()` plus `PodManifest::try_from(...)` validation, and docs now describe prompt-loader limitations without reviving ambient manifest discovery.

Validation run in the implementation worktree:

- `cargo fmt --check`
- `cargo test -p manifest profile -- --nocapture`
- `cargo test -p pod profile -- --nocapture`
- `cargo test -p client spawn -- --nocapture`
- `cargo test -p tui spawn -- --nocapture`
- `nix eval --json --file resources/nix/profiles/default.nix >/dev/null`
- `cargo test -p pod --bin insomnia-pod -- --nocapture`
- `cargo test -p pod spawn_config -- --nocapture`
- `cargo test -p manifest -p pod -p tui --lib --bins`
- `./tickets.sh doctor`
- `git diff --check`

Known unrelated full integration failures in `cargo test -p manifest -p pod -p tui` remain in existing pod rollback integration tests and were not introduced by this ticket.


---
