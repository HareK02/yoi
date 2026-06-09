---
title: "CLI: remove INSOMNIA_POD_COMMAND override"
state: "closed"
created_at: "2026-05-31T08:59:59Z"
updated_at: "2026-05-31T10:12:03Z"
---

## Background

The single-binary migration changed the normal Pod runtime command to the current `insomnia` executable plus the `pod` prefix argument. During the transition, `INSOMNIA_POD_COMMAND` remained as an executable-only development/test override.

The user decision is to remove this override now that runtime launch is aligned with the single binary. Keeping a process-wide environment override is no longer worth the configuration surface area.

## Requirements

- Remove `INSOMNIA_POD_COMMAND` support from the `insomnia` helper crate and any callers/tests.
- Keep default Pod runtime command behavior unchanged: current executable plus `pod` prefix argument.
- Update spawn/restore tests so they no longer depend on a process-wide command override.
  - Prefer a typed test injection path or direct unit tests of `PodRuntimeCommand` construction.
  - Do not introduce a replacement environment variable.
- Update docs to remove `INSOMNIA_POD_COMMAND` from supported environment variables.
- Preserve detached process behavior and `INSOMNIA-READY` handshake behavior.

## Non-goals

- Reintroducing an `insomnia-pod` binary or alias.
- Changing Pod runtime flags/profile/manifest semantics.
- Changing the Pod protocol.
- Renaming the `tui` package/crate.

## Acceptance criteria

- `git grep INSOMNIA_POD_COMMAND` finds no active code/docs references outside historical work-item records.
- Pod spawn/restore still defaults to `insomnia pod ...`.
- Focused tests cover runtime command construction without environment-variable mutation.
- `cargo fmt --check`, relevant `cargo test`/`cargo check`, `./tickets.sh doctor`, and `git diff --check` pass.
