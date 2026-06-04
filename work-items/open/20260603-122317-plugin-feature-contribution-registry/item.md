---
id: 20260603-122317-plugin-feature-contribution-registry
slug: plugin-feature-contribution-registry
title: Plugin: feature contribution registry for built-in and external capabilities
status: open
kind: feature
priority: P1
labels: [plugin, registry, tools, hooks, orchestration]
created_at: 2026-06-03T12:23:17Z
updated_at: 2026-06-04T20:23:15Z
assignee: null
legacy_ticket: null
---

## Issue

Yoi already has many capability surfaces: built-in tools, memory tools, Pod management tools, manifest permission hooks, workflow assets, notifications, and planned WorkItem / MCP / plugin features. If new features keep registering themselves through ad hoc Pod/Worker code paths, Plugin system work will not produce a single management boundary and later features such as WorkItem intake will be hard to detach.

The immediate need is not package distribution or WASM execution. The immediate need is a runtime feature contribution registry that lets built-in features and future external plugins contribute through the same existing host surfaces: Tools, Hooks, host-managed BackgroundTasks, and durable notification/history plus alert/diagnostic paths.

## Direction

Introduce a feature registry boundary for Pod runtime capability installation.

- Feature state remains owned by the feature/extension module, not by Pod history or prompt context.
- Pod interaction happens through existing surfaces:
  - Tool contributions registered into the normal ToolRegistry / permission / history path.
  - Hook contributions registered through the public Pod Hook boundary.
  - BackgroundTask contributions are host-managed for async feature work; feature modules must not create untracked runtime loops.
  - Model-visible notifications use the existing durable Notify / SystemItem / Event::SystemItem path rather than invisible context injection.
  - Transient human-facing alerts and diagnostics are separate host-defined outputs, not arbitrary plugin UI channels.
- The registry is responsible for discovery/enablement diagnostics and installation into existing surfaces; it must not create a parallel execution path.
- Built-in features should be expressible as feature contributions first. External plugin runtimes can be added later.

## Placement note

The runtime registry should initially live in the Pod layer, e.g. `crates/pod/src/feature.rs` or an equivalent module, because installation requires access to Worker tool registration, Pod Hook registry setup, manifest/profile-resolved policy, notification buffers, and Pod event bridges.

Pure descriptor types may later move to a separate `plugin` / `extension` crate if needed, but descriptor types must not depend on Pod internals.

## Requirements

- Define feature identity/source/runtime metadata for at least built-in features and future user/project plugins.
  - Example sources: builtin, user, project.
  - Example runtime kinds: builtin, declarative, mcp bridge future, wasm future.
- Define contribution descriptors or install abstractions for:
  - Tools
  - Hooks
  - BackgroundTasks
  - Model-visible notification, transient alert, and diagnostic capabilities where needed
- Define capability request / host grant data structures suitable for policy diagnostics.
- Add a registry/builder/install context that can install enabled feature contributions into existing Pod/Worker surfaces.
- Preserve current behavior while moving registration toward the registry.
  - Existing built-in tool registration must still work.
  - Existing manifest permission hook behavior must still work.
  - Existing notification/history invariants must still hold.
- Keep model-visible plugin/feature output on durable paths: tool result, committed history, or explicit notification/history append paths.
- Do not let feature contributions mutate session history, memory, prompt context, or scope outside approved host APIs.
- Document how WorkItem tools should become a built-in feature contribution rather than a special Pod context path.

## Non-goals

- Implementing WASM execution.
- Implementing plugin package discovery or archive validation.
- Implementing MCP itself.
- Implementing WorkItem management tools.
- Adding UI/TUI rendering plugins.
- Auto-enabling user/project plugins.

## Suggested phases

1. **Registry design**
   - Define feature descriptor, source, runtime kind, contribution kinds, capability request/grant, and diagnostics.
2. **Pod runtime registry skeleton**
   - Add a Pod-layer feature registry/builder and install context.
   - Keep behavior unchanged initially.
3. **Builtin registration migration**
   - Move a small, low-risk built-in feature group through the registry first.
   - Then migrate built-in tool groups / memory / Pod-management registration as appropriate.
4. **Hook integration**
   - Integrate only after `hook-public-surface-hardening` establishes a safe public Hook action surface.
5. **Follow-up alignment**
   - Update WorkItem, MCP, and plugin distribution tickets to depend on this registry boundary where they contribute capabilities.

## Acceptance criteria

- The codebase has a first-class feature contribution registry boundary for Pod runtime installation.
- At least one built-in capability group is registered through the new registry without changing behavior.
- The registry can describe Tool, Hook, and BackgroundTask contributions and records source/runtime/capability diagnostics.
- Feature installation uses existing ToolRegistry, HookRegistry, host-managed BackgroundTask lifecycle, and notification/history paths; no parallel Pod context injection path or arbitrary plugin UI channel is introduced.
- WorkItem and MCP follow-up tickets can target this registry instead of adding ad hoc registration code.
- Focused tests cover the migrated built-in registration and capability/diagnostic behavior.
