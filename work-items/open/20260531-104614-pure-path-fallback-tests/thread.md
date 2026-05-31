<!-- event: create author: tickets.sh at: 2026-05-31T10:46:14Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T10:46:51Z -->

## Plan

Preflight classification: implementation-ready.

Intent:
- Remove unnecessary process-global environment mutation from path fallback tests by making the fallback/precedence logic directly testable with explicit inputs.

Requirements:
- Start with `manifest::paths`, where path resolution is centralized and currently owns env fallback tests.
- Use small per-key pure helpers with direct optional path inputs. Do not introduce a general `PathEnv`, `EnvSnapshot`, env trait, or test-support crate.
- Keep public runtime behavior unchanged: the production wrappers still read the same env vars, ignore empty path env values as before, and resolve the same fallback order.
- Keep docs/environment.md behavior aligned with code.

Current code map:
- `crates/manifest/src/paths.rs`: core config/data/runtime/resource path resolution and env fallback tests using local `EnvGuard`.
- `docs/environment.md`: documented fallback order for home/config_dir/data_dir/runtime_dir/resource_dir.
- Other env mutations in `pod`/`pod-registry` are mostly subprocess/runtime-isolation integration setup and should not be force-fixed in this ticket.
- Provider credential env mutation belongs to `manifest-profile-encrypted-secrets`, not this ticket.

Critical risks:
- Accidentally changing path precedence while making tests pure.
- Creating an over-broad abstraction that keeps env mutation around indirectly.
- Treating spawned-process runtime isolation env setup as equivalent to pure fallback tests and breaking integration tests.

Intent packet for implementation:

Intent:
- Make `manifest::paths` fallback tests pure and independent from process env mutation.

Requirements:
- Extract narrow pure helper functions for fallback precedence.
- Convert fallback-order tests to call those helpers directly.
- Keep production path functions and documented behavior unchanged.
- Do not introduce a new crate, `PathEnv` struct, env trait, or test-only env var.

Invariants:
- Path authority remains in `manifest::paths`.
- No new environment-variable surface.
- No provider credential/env cleanup in this ticket.
- No Pod runtime/process model changes.

Non-goals:
- Eliminating all `set_var`/`remove_var` in the repository.
- Reworking resource packaging lookup beyond preserving behavior.

Escalate if:
- Removing env mutation requires changing public APIs or path semantics.
- Resource lookup behavior is ambiguous enough to need a product decision.
- Pod runtime integration tests appear to need broader runtime-dir API changes.

Validation:
- `cargo fmt --check`
- `cargo test -p manifest paths`
- relevant additional manifest tests if names differ
- `cargo check -p manifest`
- `./tickets.sh doctor`
- `git diff --check`
- report remaining `set_var`/`remove_var` in touched areas and why they remain, if any.


---

<!-- event: review author: hare at: 2026-05-31T10:54:18Z status: approve -->

## Review: approve

External reviewer: `pure-path-fallback-reviewer-20260531`
Reviewed implementation commit: `e232f54` (`test: make path fallback tests pure`)
Verdict: approve

Summary:
- `manifest::paths` fallback/precedence logic was split into private pure helpers.
- Tests now pass direct `Option<PathBuf>` inputs to verify fallback order rather than mutating process environment.
- No `PathEnv`/`EnvSnapshot`/trait/test-support crate was introduced.
- The change is limited to `crates/manifest/src/paths.rs`.

Requirements mapping:
- `config_dir`, `data_dir`, and `runtime_dir` public wrappers still read the same env keys and pass them in the existing precedence order.
- Empty env values still become `None` through the retained `env_path`/path conversion behavior.
- `docs/environment.md` fallback order matches the code.
- `git grep -n -E 'set_var\(|remove_var\(' -- crates/manifest/src/paths.rs` has no matches.
- No credential env behavior, Pod runtime/process behavior, or new env var surface was changed.

Blockers: none.
Non-blocking follow-ups: none.

Validation adequacy:
- Coder validation covered fmt, focused/full manifest tests, manifest check, doctor, diff check, and set_var/remove_var grep.
- Reviewer performed read-only diff/source/docs consistency review and grep/diff-check spot checks; no additional Cargo tests were rerun by reviewer.


---
