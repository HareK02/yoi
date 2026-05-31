---
id: 20260531-085959-eliminate-test-only-env-vars
slug: eliminate-test-only-env-vars
title: Tests: eliminate test-only environment variables
status: open
kind: task
priority: P2
labels: [test, env, cleanup]
created_at: 2026-05-31T08:59:59Z
updated_at: 2026-05-31T08:59:59Z
assignee: null
legacy_ticket: null
---

## Background

The environment-variable policy now treats process environment as an undesirable ambient input. Tests currently still use several test-only or test-generated env names, such as `INSOMNIA_TEST_*`, and many tests mutate process environment directly with local guards.

The user decision is to eliminate test-only environment-variable surfaces rather than documenting them as supported configuration. A shared test utility crate may be added if it helps remove duplicated unsafe env mutation and replace test-only env channels with typed fixtures.

## Requirements

- Remove test-only environment variables from active code/tests, including `INSOMNIA_TEST_*` patterns.
- Do not add new test-only user-facing env vars.
- Where tests need to exercise real supported env behavior, keep those mutations isolated behind a shared guard rather than ad-hoc per-test `set_var`/`remove_var` code.
- Prefer typed fixtures, temporary files, explicit config structs, or dependency injection over process-global env channels.
- It is acceptable to introduce a small `test-support` crate if it reduces duplication and keeps env mutation serialized/restored.
- Update docs so test-only env vars are not listed as a supported surface.

## Non-goals

- Removing tests that intentionally verify public path/env fallback behavior such as `INSOMNIA_HOME`, `XDG_CONFIG_HOME`, or `INSOMNIA_RUNTIME_DIR`.
- Removing credential env vars in this ticket; those belong with `manifest-profile-encrypted-secrets`.
- Removing `INSOMNIA_POD_COMMAND`; that is tracked by `remove-insomnia-pod-command-env`.

## Acceptance criteria

- No active code/tests generate or depend on `INSOMNIA_TEST_*` env names.
- Test-only env vars are absent from `docs/environment.md`.
- Any remaining test env mutation is for documented public env behavior or unavoidable external compatibility and is guarded/serialized/restored.
- If a `test-support` crate is added, it is test-only/dev-only and does not become runtime dependency surface.
- Relevant test suites pass, including tools/provider/manifest/pod tests that previously mutated env.
- `cargo fmt --check`, relevant `cargo test`/`cargo check`, `./tickets.sh doctor`, and `git diff --check` pass.
