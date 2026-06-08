---
id: 20260531-104614-pure-path-fallback-tests
slug: pure-path-fallback-tests
title: 'Tests: make path fallback tests independent from process env'
status: closed
kind: task
priority: P2
labels: [test, env, manifest, cleanup]
created_at: 2026-05-31T10:46:14Z
updated_at: 2026-05-31T10:54:49Z
assignee: null
---

## Background

The environment-variable cleanup removed test-only env surfaces such as `INSOMNIA_TEST_*` and the Pod runtime command env override. The next issue is that several tests still mutate process-global environment even when they only need to verify deterministic fallback/precedence logic.

The user clarified the intended direction:

- even env vars used by the real application do not need to be read from process env in most tests;
- tests should verify fallback/precedence behavior with direct inputs;
- do not introduce a `PathEnv`/`EnvSnapshot` abstraction or a shared test-support crate just to preserve process-env mutation;
- write the fallback helpers directly and narrowly where needed.

This ticket starts with path resolution, where the behavior is centralized and the env mutation is avoidable.

## Requirements

- In `manifest::paths`, separate fallback/precedence logic from process env reads enough that fallback tests can call pure helpers with direct `Option<PathBuf>`-style inputs.
- Prefer small per-key helpers over a general `PathEnv` struct/trait.
  - Example shape: `resolve_config_dir_from_parts(...)`, `resolve_data_dir_from_parts(...)`, `resolve_runtime_dir_from_parts(...)`, or equivalent private helpers.
  - Keep helper visibility private or `pub(crate)` only if needed by nearby tests.
- Keep runtime behavior unchanged:
  - public path functions still read the same supported env vars in the same order;
  - empty env values keep the current unset-equivalent behavior;
  - resource lookup fallback behavior remains compatible with packaging/dev use.
- Convert path fallback tests that currently use `std::env::set_var` / `remove_var` to direct helper tests where possible.
- Do not add a new helper crate or broad env abstraction.
- Do not remove credential env behavior in this ticket; that belongs to `manifest-profile-encrypted-secrets`.
- Do not attempt to eliminate subprocess integration env setup if a spawned process still needs runtime isolation; report any remaining env mutation rather than forcing risky API changes.

## Non-goals

- Removing all env reads from production code.
- Removing provider credential env support.
- Redesigning runtime directory authority or Pod process startup.
- Introducing test-support / EnvSnapshot / trait-based environment abstraction.
- Changing documented path semantics.

## Acceptance criteria

- `manifest::paths` fallback/precedence tests no longer mutate process-global environment just to test fallback order.
- The fallback order documented in `docs/environment.md` still matches the code.
- Any remaining `set_var` / `remove_var` in touched tests is either eliminated or explicitly justified in the implementation report.
- No new environment variable surface is introduced.
- `cargo fmt --check`, `cargo test -p manifest paths`, relevant affected tests, `cargo check -p manifest`, `./tickets.sh doctor`, and `git diff --check` pass.
