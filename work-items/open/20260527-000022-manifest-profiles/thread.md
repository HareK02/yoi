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

<!-- event: plan author: hare at: 2026-05-29T16:59:41Z -->

## Plan

Continue implementation beyond the merged Nix profile foundation. The previous nested delegation stopped at a reviewable vertical slice; this phase should keep iterating until the work item acceptance criteria are materially closer to completion or a real blocker is found.

Phase 2 target:

- Add profile discovery/default selection instead of requiring only explicit `--profile <path>`.
- Support discovered-name selection with source disambiguation where needed, e.g. builtin/user/project/path semantics.
- Add or substantially advance the new Pod profile selection UX so a default can be displayed as `profile: coder (default)` and changed before spawn.
- Preserve the core design: selected Nix profile resolves to a standalone manifest/config artifact; ambient user/project manifests do not merge into it.
- Keep Pod resume using resolved snapshots rather than silent Nix re-evaluation.
- Keep secrets as references only.

The delegated orchestrator should not return merely because one slice is reviewable. It should internally review acceptance criteria, delegate sub-Pods as needed, and continue with the next reachable slice unless it hits a concrete design or technical blocker. Parent-side merge/close remains reserved for this parent Pod.


---

<!-- event: review author: hare at: 2026-05-29T17:44:47Z status: approve -->

## Review: approve

Reviewed phase 2 implementation from branch `work/nix-manifest-profiles-phase2`.

Result: approved after two parent-review fixes.

Implemented scope:

- Profile registry/discovery for builtin/user/project sources.
- `[profiles]` metadata in user/project manifests for discovery/default/alias only; it is not merged into the selected runtime manifest.
- `--profile` selector parsing for explicit paths, `path:<path>`, discovered names, `default`, and source-qualified names such as `project:coder`.
- Ambiguous unqualified discovered names fail closed.
- TUI fresh-spawn UI now shows a selectable `profile:` row, uses discovered choices, marks defaults, and includes `manifest cascade` as opt-out.
- SpawnConfig passes selected profiles to `insomnia-pod --profile`; resume/attach paths do not re-evaluate profiles.
- Docs and focused tests updated.

Parent review findings fixed by child orchestrator:

1. Unqualified alias targets initially resolved globally. Fixed so aliases declared in a source resolve unqualified targets within that declaring source by default.
2. Defaults pointing at aliases initially did not mark the resolved target entry as default, causing TUI to fall back to `manifest cascade`. Fixed by resolving the default through `select_named()` before setting `is_default` flags.

Validation run by parent reviewer:

- `cargo fmt --check`
- `cargo check`
- `cargo test -p manifest profile -- --nocapture`
- `cargo test -p tui spawn -- --nocapture`
- `cargo test -p pod profile -- --nocapture`
- `cargo test -p client spawn -- --nocapture`
- `git diff --check`

All passed. Full `cargo test` was run by the child orchestrator and failed only in the unrelated existing/flaky `llm-worker` parallel timing test class.

Remaining polish/follow-up candidates, not blockers for this work item:

- A richer popup-style profile picker instead of inline cycling.
- Actual bundled builtin profile files once default builtin semantics are decided.
- `nix eval` timeout/robustness follow-up.
- Encrypted secret store integration remains tracked by the related encrypted-secrets work item.


---
