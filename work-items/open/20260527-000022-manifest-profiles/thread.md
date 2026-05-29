<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:22Z -->

## Migrated

Migrated from TODO.md entry without a legacy ticket file. No legacy review file was present at migration time.

---

<!-- event: plan author: hare at: 2026-05-29T16:09:27Z -->

## Plan

Implementation will proceed through a child orchestrator Pod in a dedicated worktree as an experiment in nested Pod delegation.

Initial implementation target:

- Introduce Nix profile resolution as a new manifest source before the existing manifest cascade.
- Start with explicit path-based profiles; discovered-name/default selection and rich TUI picker can be staged after the core resolver if necessary.
- Provide a minimal bundled Nix helper that can produce a typed resolved manifest/config artifact.
- Keep existing TOML manifest loading as compatibility/debug/test infrastructure.
- Persist enough profile identity and resolved snapshot data for future restore semantics; do not silently re-evaluate profiles on resume.
- Secret values must remain references only; plaintext secrets are out of scope for the profile resolver.

The child orchestrator may split implementation among sub-Pods, but final merge/close remains parent-side.


---

<!-- event: review author: hare at: 2026-05-29T16:52:47Z status: approve -->

## Review: approve

Reviewed the nested Pod implementation from branch `work/nix-manifest-profiles`.

Result: approved after blocking fix.

Findings:

- Initial review found one blocking issue: `--profile` rejected non-empty `INSOMNIA_USER_MANIFEST`. That contradicted the profile design because a selected Nix profile is a standalone runtime config artifact and ambient user/project manifest cascade should not affect it.
- The child orchestrator fixed this in commit `c9a175a fix: ignore user manifest for profiles` and added `profile_ignores_non_empty_user_manifest_env` without invoking real Nix.
- Profile foundation is intentionally a vertical slice: explicit path-based Nix profile resolution, minimal Nix helper, CLI/TUI spawn entrypoints, resolved snapshot metadata, and restore-from-snapshot behavior. Rich discovery/default picker remains future work.

Validation run by reviewer:

- `cargo fmt --check`
- `cargo test -p manifest profile -- --nocapture`
- `cargo test -p pod --bin insomnia-pod profile -- --nocapture`
- `cargo check -p session-store -p manifest -p pod -p client -p tui -p provider`
- `cargo check -p pod -p tui`
- `git diff --check`
- Manual `nix eval --json --file` smoke check for `resources/nix/profile-lib.nix`

Non-blocking follow-up candidates:

- Hide or narrow `ResolvedProfile::raw_artifact` if future call sites might log/persist accidental raw Nix output.
- Add a timeout around `nix eval` so profile startup cannot hang indefinitely.
- Validate direct `client::SpawnConfig` construction that combines `profile_path` with `resume_from`; TUI currently avoids it.
- Build richer profile discovery/default selection and the full TUI profile picker.


---
