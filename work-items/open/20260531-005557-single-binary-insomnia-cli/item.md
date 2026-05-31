---
id: 20260531-005557-single-binary-insomnia-cli
slug: single-binary-insomnia-cli
title: CLI: clarify single-binary insomnia architecture
status: open
kind: task
priority: P2
labels: [cli, architecture, nix]
created_at: 2026-05-31T00:55:57Z
updated_at: 2026-05-31T00:56:38Z
assignee: null
legacy_ticket: null
---

## Background

The installed user-facing command is already `insomnia`, while the Cargo package/crate that owns it is still named `tui`. Headless CLI commands such as `insomnia memory lint` are starting to live in that same binary. This is acceptable short-term, but the architecture should be made explicit before more commands accumulate.

The preferred product direction is a single user-facing `insomnia` binary that can run TUI screens and headless commands. The main downside is that headless commands inherit ratatui/crossterm and related dependencies, but binary size is not expected to dominate runtime memory compared with LLM clients, HTTP/TLS, JSON/session/memory processing, and spawned process orchestration. A single binary also improves distribution and “minimal standalone install” ergonomics.

## Requirements

- Decide and document the intended command architecture:
  - one installed `insomnia` binary as the primary user-facing entry point;
  - headless subcommands under that binary;
  - TUI/interactive paths as subcommands or default modes;
  - relationship to `insomnia-pod`.
- Refactor only if necessary to keep headless commands cleanly separated from terminal/TUI initialization.
- Ensure headless commands do not initialize ratatui, enter raw mode, connect Pod sockets, or spawn Pod processes unless explicitly requested by that command.
- Evaluate whether to rename the Cargo package/crate from `tui` to `insomnia`.
  - If the rename is straightforward, implement it with scoped Nix/docs updates.
  - If it is broad/risky, leave the rename as a follow-up and document the boundary.
- Keep installed command names stable unless a separate explicit decision says otherwise.
- Preserve Nix/Home Manager packaging expectations: installing the flake package should put the intended commands on `PATH`.
- Avoid introducing separate binaries for every headless command unless there is a measured startup/binary-size/runtime-memory reason.

## Non-goals

- Reworking the Pod daemon binary protocol.
- Removing `insomnia-pod` unless a separate architecture decision covers Pod process lifecycle.
- Feature-gating ratatui/crossterm for a headless-only build before there is a measured need.
- Large CLI UX redesign beyond clarifying the single-binary structure.

## Acceptance criteria

- The repository has a clear documented decision or implementation path for the single-binary `insomnia` CLI architecture.
- Existing TUI flows and headless command flows remain tested.
- If crate/package rename is implemented, Cargo/Nix/docs references are updated consistently.
- If crate/package rename is deferred, a precise follow-up or documented rationale explains why.
- `cargo fmt --check`, relevant `cargo test`/`cargo check`, `./tickets.sh doctor`, and `git diff --check` pass.
