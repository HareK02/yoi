<!-- event: create author: tickets.sh at: 2026-05-31T11:08:18Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T11:28:41Z -->

## Plan

Read-only investigator: `resource-dir-removal-investigator-20260531`
Classification: implementation-ready

Conclusion:
- `manifest::paths::resource_dir()` can be removed.
- `INSOMNIA_RESOURCE_DIR` can be removed as an active supported environment variable after replacing the two remaining runtime filesystem resource dependencies.
- Builtin assets should be embedded rather than discovered through installed `share/insomnia/resources`.

Current code map:
- `crates/manifest/src/paths.rs`
  - `RESOURCE_DIR_ENV = "INSOMNIA_RESOURCE_DIR"`
  - `resource_dir()` checks env, installed `$prefix/share/insomnia/resources`, then source-tree `resources/`.
  - `builtin_profiles_dir()` returns `resource_dir()/profiles`.
- `crates/manifest/src/profile.rs`
  - builtin profile discovery still scans `paths::builtin_profiles_dir()`.
  - `builtin_model_context_window()` still reads `resource_dir()/models/builtin.toml`.
- `crates/provider/src/catalog.rs`
  - provider/model builtin catalogs are already embedded with `include_str!`.
- `crates/pod/src/prompt/loader.rs` and `catalog.rs`
  - builtin prompts are already embedded with `include_dir!` / `include_str!`.
- `package.nix` still installs `resources` to `$out/share/insomnia/resources` and checks prompt dir existence.
- `docs/environment.md` and `docs/nix.md` still document runtime resource directory/package resources.

Embedded vs filesystem map:
- Already runtime-embedded:
  - `resources/prompts/**`
  - `resources/providers/builtin.toml`
  - `resources/models/builtin.toml` in provider crate.
- Still runtime-filesystem-dependent:
  - `resources/profiles/default.lua` through builtin profile directory scan.
  - manifest-side model context lookup through `resource_dir()/models/builtin.toml`.
- Should remain filesystem-based:
  - user/project `profiles.toml` and profile paths;
  - config prompt/model/provider overrides;
  - explicit `--manifest <path>` and explicit filesystem profile path selectors.

Recommended implementation:
1. Remove `RESOURCE_DIR_ENV`, `resource_dir()`, and `builtin_profiles_dir()` from `manifest::paths`.
2. Embed builtin Lua profiles explicitly in `manifest::profile`, starting with `resources/profiles/default.lua`.
3. Preserve profile selector semantics for `default`, `builtin:default`, user/project source-qualified selectors, explicit paths, and ambiguity failures.
4. Use synthetic builtin provenance/diagnostics rather than fake filesystem paths.
5. Keep host Lua modules unchanged. Do not support embedded local filesystem require by falling back to `resources/profiles`; current default profile does not need local require.
6. Replace manifest-side `builtin_model_context_window()` filesystem read with `include_str!("../../../resources/models/builtin.toml")`. Do not make `manifest` depend on `provider` because `provider -> manifest` already exists.
7. Remove Nix runtime install/check for `$out/share/insomnia/resources` after runtime code no longer needs it.
8. Remove `INSOMNIA_RESOURCE_DIR` / `resource_dir` docs from `docs/environment.md`; update Nix/profile docs where they describe installed runtime resources.

Critical risks:
- Changing serialized profile provenance shape for builtin profiles. This is acceptable if tests/docs are updated, but do not pretend embedded profiles have real paths.
- Embedded profile local `require`: current builtin default has none. If future builtin profiles need it, add an explicit embedded module map; do not retain filesystem fallback by inertia.
- Manifest/provider catalog dependency cycle: do not call provider builtin APIs from manifest in this ticket.
- Keep source-tree `resources/` in the Nix build source closure because compile-time `include_str!` / `include_dir!` still need it; only installed runtime resources should disappear.

Implementation-ready intent packet:

Intent:
- Remove runtime filesystem resource discovery for bundled assets and delete `INSOMNIA_RESOURCE_DIR` as an active configuration surface.

Requirements:
- Embed builtin Lua profile(s) and manifest-side builtin model context lookup.
- Preserve existing profile selection behavior and user/project profile semantics.
- Remove runtime `resource_dir` path API and Nix installed resources dependency.
- Update docs to reflect embedded builtin assets and remove `INSOMNIA_RESOURCE_DIR`.

Invariants:
- `config_dir` / user profile / project profile semantics remain unchanged.
- No ambient manifest discovery is restored.
- No `INSOMNIA_RESOURCE_DIR` compatibility path is kept.
- `manifest` must not depend on `provider`.
- Pod runtime remains separate and prompt embedding behavior remains intact.

Non-goals:
- Credential env cleanup.
- Reworking provider catalog ownership into a shared crate.
- Changing user/project config override paths.

Escalate if:
- Builtin profile embedding requires local filesystem require support beyond current default profile.
- A public API requires real builtin profile paths rather than provenance labels.
- Nix packaging unexpectedly needs installed resources for runtime behavior after embedding.

Validation:
- grep for `INSOMNIA_RESOURCE_DIR|RESOURCE_DIR_ENV|resource_dir\(|builtin_profiles_dir\(` in active code/docs/package files.
- `cargo fmt --check`
- `cargo test -p manifest profile`
- focused tests for builtin default profile and compact ratio known-model context
- `cargo test -p provider catalog`
- `cargo test -p pod prompt`
- `cargo check -p manifest -p provider -p pod -p tui`
- `nix build .#insomnia` and install-output checks: `bin/insomnia` exists, `bin/insomnia-pod` absent, `share/insomnia/resources` absent if removed, `insomnia pod --help` works.
- `./tickets.sh doctor`
- `git diff --check`


---

<!-- event: review author: hare at: 2026-05-31T11:54:28Z status: approve -->

## Review: approve

External reviewer: `remove-resource-dir-reviewer-20260531`
Reviewed implementation commit: `365ec8b7fad008ab36bdc4de3adadb3696739a07` (`manifest: embed builtin resources`)
Verdict: approve

Summary:
- Runtime filesystem resource discovery and `INSOMNIA_RESOURCE_DIR` active surface were removed.
- Builtin profile is embedded and represented as `builtin:default` without pretending to have a runtime filesystem path under `resources/profiles`.
- User/project/path profile semantics remain filesystem-backed.
- Builtin model context lookup is embedded on the manifest side without adding a `manifest -> provider` dependency.
- Nix package/docs no longer require installed runtime resources.

Requirements mapping:
- `INSOMNIA_RESOURCE_DIR`, `RESOURCE_DIR_ENV`, `resource_dir()`, and `builtin_profiles_dir()` have no active hits in `crates/`, `docs/`, or `package.nix`.
- Embedded builtin profiles do not silently support local filesystem `require` by falling back to resources; non-filesystem profile local modules produce the existing disabled diagnostic.
- `crates/pod/src/spawn/tool.rs` change is test/API fallout for `ProfileDiscovery::with_sources`, not a runtime/protocol behavior change.
- Runtime installed resources are no longer required; source `resources/` remains build-time input for embeds.

Blockers: none.

Non-blocking follow-ups:
- Add a narrow regression test for an embedded builtin-like profile attempting local `require("x")`, asserting the explicit disabled diagnostic.
- Rerun `nix build .#insomnia` in an environment with enough disk space before treating the ticket as fully release-validated; the final rerun is environment-blocked by disk full.

Validation adequacy:
- Rust validation and forbidden-symbol grep are well targeted.
- Final Nix validation remains pending due to `No space left on device`, not due to a code review finding.


---
