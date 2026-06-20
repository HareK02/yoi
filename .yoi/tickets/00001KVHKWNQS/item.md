---
title: 'Plugin: add authoring CLI new/check/pack'
state: 'ready'
created_at: '2026-06-20T04:16:14Z'
updated_at: '2026-06-20T04:17:24Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'cli', 'authoring', 'templates', 'package-validation', 'packaging', 'read-only-check']
---

## Background

After the Rust PDK and embedded templates exist, independent Plugin developers need first-party CLI tooling to scaffold, validate, and package Plugins without relying on remote shell scripts or hand-written ZIP commands.

This Ticket adds authoring commands for local Plugin development. It complements read-only operational inspection (`yoi plugin list/show`) but serves a different audience: Plugin authors preparing a package before enabling it in Yoi.

## Requirements

- Add Plugin authoring subcommands to the product CLI.
  - `yoi plugin new rust-component-tool <path-or-name>`
  - `yoi plugin check <path-or-package>`
  - `yoi plugin pack <path> [--output <file>]`
- `new` uses embedded templates only.
  - No remote template fetch.
  - No `curl | sh` flow.
  - Generated files should include `Cargo.toml`, `src/lib.rs`, `plugin.toml`, and README/next-steps.
  - Generated dependency should be appropriate for the current checkout/release mode: local path in checkout mode, or documented git rev/tag pattern if out-of-tree.
- `check` validates a Plugin directory or `.yoi-plugin` package without executing Plugin code.
  - Parse `plugin.toml`.
  - Validate package id/version/source-compatible shape.
  - Validate runtime kind and referenced artifact presence.
  - Validate Component Model world metadata where possible.
  - Validate Tool schema shape.
  - Validate requested permissions / host API declarations.
  - Validate archive safety for packages: path traversal, root escape, unsupported compression, bounded file count/size.
  - Calculate and print deterministic digest.
  - Produce actionable diagnostics and a suggested enablement/grant snippet.
- `pack` creates a deterministic `.yoi-plugin` package.
  - Include required manifest/runtime files.
  - Use currently supported archive format, including stored entries if compression is not supported.
  - Reject unsafe paths / root escapes.
  - Print output path and digest.
  - Do not modify workspace enablement config.
- Provide JSON output for automation where useful.
  - At minimum `check --json` and `pack --json`.
- Keep commands safe.
  - `check` and `pack` do not execute Plugin code.
  - `new` only writes into the requested destination and refuses to overwrite non-empty directories unless an explicit safe option is added.
  - No secrets are generated or embedded.
- Integrate with existing inspection language.
  - Diagnostics/statuses should align with `yoi plugin list/show` where possible.

## Acceptance criteria

- `yoi plugin new rust-component-tool ./my-plugin` creates a usable template without network access.
- `yoi plugin check ./my-plugin` validates the generated template and reports next steps/digest/enablement guidance.
- `yoi plugin pack ./my-plugin` creates a `.yoi-plugin` package that Yoi discovery can read.
- `check` can validate an existing `.yoi-plugin` archive.
- `check --json` returns a stable typed report suitable for tests/agents.
- `pack --json` returns output path and digest.
- Unsafe package paths / traversal / unsupported compression / missing manifest / missing runtime artifact are rejected with clear diagnostics.
- Commands do not execute Plugin code or mutate enablement config.
- Tests cover:
  - `new` generated file set;
  - refusal to overwrite non-empty destination;
  - `check` valid directory;
  - `check` invalid manifest;
  - `check` missing runtime artifact;
  - `check` unsafe package archive;
  - `pack` deterministic digest;
  - `pack` package is discoverable by existing Plugin discovery;
  - JSON report shape.
- Validation: focused CLI/plugin authoring tests, relevant `cargo check` / `cargo test`, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` because product CLI/resources/packaging behavior changes.

## Non-goals

- Remote template fetching.
- Publishing or installing from a registry.
- Enabling/disabling Plugins in workspace config.
- Executing Plugin code during `check`.
- crates.io publication of PDK.
- Service / Ingress scaffolding.
- Multi-language templates beyond Rust Component Model Tool.

## Related work

- `00001KVG0HR9M` — Objective: Plugin platform roadmap.
- `00001KVHKWNQA` — Rust PDK and embedded authoring templates.
- `00001KVFD3YSV` — Plugin read-only CLI inspection list/show.
- `00001KVG0HR96` — Plugin Component Model runtime.
- `docs/development/plugin-development.md` — current Plugin development guide.
