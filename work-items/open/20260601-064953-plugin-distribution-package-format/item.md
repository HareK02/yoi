---
id: 20260601-064953-plugin-distribution-package-format
slug: plugin-distribution-package-format
title: Plugin distribution package format and discovery
status: open
kind: feature
priority: P2
labels: [plugin, distribution, workspace, user]
created_at: 2026-06-01T06:49:53Z
updated_at: 2026-06-01T06:50:33Z
assignee: null
legacy_ticket: null
---

## Background

The plugin extension surface ticket (`plugin-extension-surface`) defines Plugins as a safe contribution model for Tools and Hooks, with MCP, declarative hooks, and WASM treated as runtime mechanisms. The next design question is how plugins are distributed, discovered, installed, and enabled across user and workspace scopes.

The desired initial direction is a single-file plugin package that can be placed in user or workspace plugin stores, for example:

- `~/.config/insomnia/plugins/<id>.insomnia-plugin`
- `./.insomnia/plugins/<id>.insomnia-plugin`

The package should be easy to copy, inspect, cache, and pin, while preserving Insomnia's scope, permission, history, prompt-context, and trust invariants. In particular, workspace plugins may come from a repository checkout and must not execute merely because an archive exists under `./.insomnia/plugins`.

## Requirements

- Define a first-class plugin package format.
  - Use a single archive file with an Insomnia-specific extension such as `.insomnia-plugin`.
  - Require a root plugin manifest file such as `plugin.toml`.
  - Support packaged assets such as `module.wasm`, JSON schemas, README, and license files.
  - Specify archive safety rules, including path traversal rejection, bounded extraction, and deterministic digest calculation.
- Define plugin stores and source/trust mapping.
  - User plugin store: `~/.config/insomnia/plugins/`.
  - Workspace/project plugin store: `./.insomnia/plugins/`.
  - Map stores to the existing source vocabulary (`User`, `Project`/workspace, and future `Builtin`).
  - Treat `user:<id>` and `project:<id>` as distinct plugin references; ambiguous unqualified IDs should fail closed.
- Separate discovery from enablement.
  - Insomnia may discover plugin packages in configured stores.
  - Discovered packages must not register Tools/Hooks, initialize WASM, or start MCP servers until explicitly enabled by manifest/profile configuration.
  - Enablement must resolve package identity, version/API compatibility, source, digest, requested capabilities, and host-granted capabilities.
- Define package manifest semantics.
  - Include fields for plugin id, version, plugin API version, runtime kind, source/provenance, metadata, contributed tools/hooks, configuration schema, and requested capabilities.
  - Capability declarations are requests, not grants; effective grants remain controlled by manifest/profile policy, scope, permissions, web/network policy, secret references, and runtime-specific allowlists.
- Define runtime-specific packaging expectations.
  - Declarative hooks can be packaged as config-only assets without arbitrary code execution.
  - WASM plugins should package a module plus schemas/assets and run only with explicit host imports/capabilities.
  - MCP should remain modeled as a backend/bridge; packaging an MCP server or process command requires explicit process/capability design and must not auto-start from workspace packages.
- Define cache/pinning behavior.
  - Extract or materialize packages into a digest-keyed cache before runtime initialization.
  - Consider digest pins in manifest/profile enablement entries or a future lock file.
  - Record resolved package digest/source/provenance in the resolved manifest/session metadata where appropriate.
- Define diagnostics.
  - Report load/parse/compatibility/capability/runtime failures with plugin id, source, runtime kind, and phase.
  - Diagnostics must not expose secret values, raw credentials, or unsafe command/environment details.

## Non-goals

- Implementing the full plugin runtime system in this ticket.
- Implementing a package registry or network installer.
- Auto-enabling plugins solely because they are present in a plugin directory.
- Defining UI/TUI rendering extension packaging.
- Allowing arbitrary host scripting languages as plugin packages.
- Starting workspace-provided MCP servers without explicit enablement and capability approval.

## Suggested phases

1. **Architecture note**
   - Define `.insomnia-plugin` package structure, `plugin.toml` fields, source/trust model, discovery vs enablement, archive safety, cache/digest behavior, and runtime mappings.
2. **Manifest/profile config shape**
   - Add or propose `[plugins]` enablement entries with source/id/version/digest selectors and capability grants.
3. **Package discovery prototype**
   - Implement read-only discovery of user/workspace plugin packages and diagnostics without runtime initialization.
4. **Package validation and cache**
   - Validate archive layout, parse `plugin.toml`, compute digest, and materialize into a digest-keyed cache.
5. **Registry integration**
   - Connect validated packages to the plugin contribution registry from `plugin-extension-surface` follow-up work.
6. **Runtime-specific follow-ups**
   - Split declarative hook packaging, WASM packaging, and MCP packaging/bridge behavior into separate tickets as needed.

## Acceptance criteria

- The repository has a documented plugin distribution/package proposal covering user and workspace plugin stores, single-file archive format, manifest fields, archive safety, cache/digest behavior, and discovery vs enablement.
- The proposal explicitly states that placing a package in `~/.config/insomnia/plugins/` or `./.insomnia/plugins/` is discovery only, not execution or registration.
- The design maps package sources to user/project/builtin trust categories and defines how ID collisions and ambiguous selectors are handled.
- The design explains how capability requests differ from host-granted capabilities and how existing scope/permission/secret/web policy remains authoritative.
- Runtime-specific notes cover declarative hooks, WASM packages, and MCP backend/bridge packaging constraints.
- Follow-up implementation tickets can be cut independently for manifest/profile enablement, package discovery, archive validation/cache, WASM packaging, and MCP packaging alignment.
- Any code changes in this ticket, if taken beyond design docs, are limited to safe internal boundaries and focused tests.
