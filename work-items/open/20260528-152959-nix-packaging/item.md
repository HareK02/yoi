---
id: 20260528-152959-nix-packaging
slug: nix-packaging
title: Package Insomnia with Nix
status: open
kind: task
priority: P2
labels: [packaging, nix, distribution]
created_at: 2026-05-28T15:29:59Z
updated_at: 2026-05-28T15:29:59Z
assignee: null
legacy_ticket: null
---

## Background

Insomnia should be easy to install and run on Nix/NixOS systems without requiring each user to hand-roll a local derivation. Add a Nix packaging entry point suitable for development and user installation.

This ticket is about packaging and installability, not changing runtime behavior. The package should build the Rust workspace binaries and include the runtime resources needed by the installed commands.

## Requirement

- Add Nix packaging for the repository.
  - Prefer a `flake.nix` entry point unless there is a strong project-local reason not to.
  - Provide package outputs for the user-facing binaries, at minimum the Pod CLI and TUI binaries produced by the workspace.
  - Provide a dev shell or equivalent developer environment if it can be done without large scope creep.
- Ensure runtime resources are included or discoverable.
  - Built-in prompts/resources required at runtime must be packaged in the derivation output.
  - Installed binaries should not rely on the source checkout layout unless explicitly running in development mode.
- Keep local/user configuration separate from packaged resources.
  - Packaging should not bake user manifests, provider keys, sessions, memory, or runtime state into the derivation.
  - Existing XDG / `INSOMNIA_*` path behavior should remain the source of user config/data/runtime locations.
- Make the package reproducible and CI-friendly.
  - Pin inputs through the flake lock if a flake is used.
  - Avoid network access during the build.
  - Vendor or hash Cargo dependencies through normal Nix Rust packaging mechanisms.
- Document usage.
  - How to build the package.
  - How to run TUI/Pod binaries from Nix.
  - How user config is discovered.
  - Known limitations.

## Acceptance criteria

- `nix build` or the documented equivalent builds the package from a clean checkout.
- Installed binaries can find built-in resources/prompts at runtime.
- User config/data/runtime paths continue to resolve through existing path logic and are not stored in the Nix store.
- A minimal smoke test or check verifies at least command startup/help/version without requiring real provider credentials.
- Documentation exists for Nix users.
- Packaging files are formatted by the relevant Nix formatter if one is adopted.
- `cargo fmt --check`
- Existing Rust checks affected by packaging changes still pass, or packaging-only validation is clearly documented.

## Out of scope

- Publishing to nixpkgs.
- NixOS module / Home Manager module.
- Packaging external LLM providers or model runtimes.
- Secret management for provider API keys.
- Changing manifest/path semantics specifically for Nix unless a separate design decision is made.
