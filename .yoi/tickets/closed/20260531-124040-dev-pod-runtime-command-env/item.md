---
id: 20260531-124040-dev-pod-runtime-command-env
slug: dev-pod-runtime-command-env
title: 'Dev: add Pod runtime executable override env'
status: closed
kind: task
priority: P2
labels: [cli, pod, env, dev]
created_at: 2026-05-31T12:40:40Z
updated_at: 2026-05-31T20:41:56Z
assignee: null
legacy_ticket: null
---

## Background

During dogfooding, long-running TUI/Pod processes can keep executing an old `target/debug/insomnia` inode after `cargo build` or `cargo clean` replaces/removes the binary. On Linux, `std::env::current_exe()` then returns a path like:

```text
/home/.../target/debug/insomnia (deleted)
```

Current Pod spawn/restore paths use `current_exe() + ["pod"]` as the runtime command, so a long-running process can try to spawn `.../insomnia (deleted) pod` and fail with `No such file or directory`.

This is primarily a development/dogfooding problem. The previous general-purpose `INSOMNIA_POD_COMMAND` override was intentionally removed; do not restore that semantics. Instead, add a narrow development escape hatch that replaces only the executable used in the standard `insomnia pod ...` runtime command.

## Requirements

- Add a narrowly scoped development override environment variable, tentatively `INSOMNIA_POD_RUNTIME_COMMAND`.
- Semantics:
  - if unset/empty, default remains `current_exe() + ["pod"]`;
  - if set, the value is the executable path used instead of `current_exe()`;
  - the `pod` prefix argument is still automatically added;
  - the value is not shell-parsed and cannot include arguments.
- Keep this as a development escape hatch, not normal user configuration.
- Update diagnostics so a failed spawn shows the resolved runtime command clearly enough to debug deleted-path issues.
- Document the variable in `docs/environment.md` under a clearly development-only section.
- Do not reintroduce `INSOMNIA_POD_COMMAND` or its old executable-without-prefix semantics.
- Do not change Pod runtime flags/profile/manifest/protocol semantics.
- This ticket may be implemented before or folded into `insomnia-crate-cli-owner`, but must not create `pod -> insomnia` or `tui -> insomnia` architectural coupling beyond the current transitional state.

## Non-goals

- Making environment variables a general configuration mechanism.
- Adding shell-string command parsing.
- Supporting wrapper commands with arguments.
- Solving all hot-reload/self-rebuild lifecycle issues.
- Reintroducing the removed `insomnia-pod` binary/alias.

## Acceptance criteria

- Setting `INSOMNIA_POD_RUNTIME_COMMAND=/path/to/insomnia` causes spawn/restore paths to execute `/path/to/insomnia pod ...`.
- Unset or empty `INSOMNIA_POD_RUNTIME_COMMAND` preserves the default `current_exe() + ["pod"]` behavior.
- Tests cover override, empty-as-unset, and no shell parsing/argument splitting semantics.
- Active code/docs do not reference the old `INSOMNIA_POD_COMMAND` name except historical work items.
- `docs/environment.md` explains this as a development-only escape hatch for dogfooding/rebuild deleted-exe cases.
- `cargo fmt --check`, focused tests for the runtime command helper/spawn paths, `cargo check` for affected crates, `./tickets.sh doctor`, and `git diff --check` pass.
