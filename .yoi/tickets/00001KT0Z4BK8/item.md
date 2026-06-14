---
title: 'Plugin distribution package format and discovery'
state: 'done'
created_at: '2026-06-01T06:49:53Z'
updated_at: '2026-06-14T15:56:45Z'
queued_by: 'workspace-panel'
queued_at: '2026-06-14T15:40:15Z'
---

## Background

The plugin extension surface ticket (`00001KSXRQ4G8`) defines Plugins as the user-facing package/config/runtime layer that uses the `pod::feature` API substrate to contribute Tools, Hooks, and related runtime surfaces. The next design question is how plugins are distributed, discovered, installed, and enabled across user and workspace scopes.

MCP is intentionally not modeled as a Plugin package/runtime in this Ticket. MCP is a separate feature-backed protocol integration with its own enablement and trust policy. Plugin package design may later reference MCP-related assets only through an explicitly approved follow-up; package discovery must not imply MCP server execution.

The desired initial direction is a single-file plugin package that can be placed in user or workspace plugin stores, for example:

- `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/<id>.yoi-plugin`
- `<workspace>/.yoi/plugins/<id>.yoi-plugin`

The package should be easy to copy, inspect, cache, and pin while preserving Yoi's scope, permission, history, prompt-context, and trust invariants. Workspace plugins may come from a repository checkout and must not execute merely because an archive exists under `./.yoi/plugins`.

## Requirements

- Define a first-class plugin package format.
  - Use a single archive file with a Yoi-specific extension such as `.yoi-plugin`.
  - Require a root plugin manifest file such as `plugin.toml`.
  - Support packaged assets such as `module.wasm`, JSON schemas, README, and license files.
  - Specify archive safety rules, including path traversal rejection, bounded extraction, and deterministic digest calculation.
- Define plugin stores and source/trust mapping.
  - User plugin store: `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/`.
  - Workspace/project plugin store: `<workspace>/.yoi/plugins/`.
  - Builtin plugin source: Yoi-distributed `builtin:` registry.
  - Map stores to the source vocabulary (`user`, `project`, `builtin`).
  - Treat `user:<id>` and `project:<id>` as distinct plugin references; ambiguous unqualified IDs fail closed.
- Separate discovery from enablement.
  - Yoi may discover plugin packages in configured stores.
  - Discovered packages must not register Tools/Hooks, initialize WASM, start services, or start MCP servers until explicitly enabled by manifest/profile configuration.
  - Enablement must resolve package identity, version/API compatibility, source, digest, requested Plugin permissions, and effective Plugin-layer grants.
- Define package manifest semantics.
  - Include fields for plugin id, version, plugin API version, runtime kind, source/provenance, metadata, contributed tool/hook descriptors, configuration schema, and requested Plugin permissions.
  - Permission declarations are requests, not grants.
  - Effective grants are controlled by Plugin-layer policy and existing Yoi manifest/profile policy, scope, tool permissions, web/network policy, secret references, and runtime-specific allowlists.
  - Do not use `pod::feature` `HostAuthority` / authority grants as the Plugin package permission model.
- Define runtime-specific packaging expectations.
  - Declarative hooks can be packaged as config-only assets without arbitrary code execution.
  - WASM plugins should package a module plus schemas/assets and run only with explicit host imports/capabilities decided by Plugin-layer policy.
  - MCP is separate from Plugin packaging for now; packaging or launching an MCP server from a plugin package is out of scope unless a later Ticket defines that bridge explicitly.
- Define cache/pinning behavior.
  - Extract or materialize packages into a digest-keyed cache before runtime initialization.
  - Consider digest pins in manifest/profile enablement entries or a future lock file.
  - Record resolved package digest/source/provenance in the resolved manifest/session metadata where appropriate.
- Define diagnostics.
  - Report load/parse/compatibility/permission/runtime failures with plugin id, source, runtime kind, and phase.
  - Diagnostics must not expose secret values, raw credentials, or unsafe command/environment details.

## Non-goals

- Implementing the full plugin runtime system in this ticket.
- Implementing a package registry or network installer.
- Auto-enabling plugins solely because they are present in a plugin directory.
- Defining UI/TUI rendering extension packaging.
- Allowing arbitrary host scripting languages as plugin packages.
- Using `pod::feature` authority grants as the Plugin package permission model.
- Starting workspace-provided MCP servers from plugin package discovery.

## Suggested phases

1. **Architecture note**
   - Define `.yoi-plugin` package structure, `plugin.toml` fields, source/trust model, discovery vs enablement, archive safety, cache/digest behavior, and runtime mappings.
2. **Manifest/profile config shape**
   - Add or propose `[plugins]` enablement entries with source/id/version/digest selectors and Plugin-layer permission grants.
3. **Package discovery prototype**
   - Implement read-only discovery of user/workspace plugin packages and diagnostics without runtime initialization.
4. **Package validation and cache**
   - Validate archive layout, parse `plugin.toml`, compute digest, and materialize into a digest-keyed cache.
5. **Feature API integration**
   - Connect enabled, validated Plugin runtime contributions to `pod::feature` as the API substrate.
6. **Runtime-specific follow-ups**
   - Split declarative hook packaging, WASM packaging, and any external process/plugin bridge behavior into separate tickets as needed.
   - Keep MCP integration in its own Ticket family unless a future explicit bridge is approved.

## Acceptance criteria

- The repository has a documented plugin distribution/package proposal covering user and workspace plugin stores, single-file archive format, manifest fields, archive safety, cache/digest behavior, and discovery vs enablement.
- The proposal explicitly states that placing a package in `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/` or `<workspace>/.yoi/plugins/` is discovery only, not execution or registration.
- The design maps package sources to user/project/builtin trust categories and defines how ID collisions and ambiguous selectors are handled.
- The design explains Plugin permission requests/grants as Plugin-layer policy, distinct from `pod::feature` authority/grant concepts.
- Runtime-specific notes cover declarative hooks and WASM packages; MCP is called out as a separate feature-backed integration, not part of initial Plugin packaging.
- Follow-up implementation tickets can be cut independently for manifest/profile enablement, package discovery, archive validation/cache, Plugin permission policy, WASM packaging, and any future MCP/plugin bridge alignment.
- Any code changes in this ticket, if taken beyond design docs, are limited to safe internal boundaries and focused tests.
