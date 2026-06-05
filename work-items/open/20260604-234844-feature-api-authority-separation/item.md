---
id: 20260604-234844-feature-api-authority-separation
slug: feature-api-authority-separation
title: Feature API: separate internal modules from external-plugin authority model
status: open
kind: task
priority: P1
labels: [plugin, feature-registry, permissions, architecture]
created_at: 2026-06-04T23:48:44Z
updated_at: 2026-06-05T00:49:53Z
assignee: null
legacy_ticket: null
---

## Issue

The first feature-registry slice intentionally placed contribution descriptors and future external-plugin host authorities in the same `pod::feature` module. That was acceptable as a scaffold, but it risks making simple internal module extraction look like it participates in the external-plugin permission model.

For internal modules that are merely moved outside a Pod implementation file, the registry should act as an API/registration boundary: descriptor-declared contributions, install reports, and normal ToolRegistry/Hook paths. It should not require or imply sandbox/user-approval authorities unless the module asks the host for dangerous external/plugin-style APIs.

## Direction

Separate the concepts in code and naming:

- **Contribution boundary**: tools, hooks, background task declarations, service providers, descriptor/digest locking, duplicate checks, install reports.
- **Internal built-in module boundary**: in-process modules constructed by the host with explicit Rust handles such as `TaskStore`; normally no host-authority request/grant flow.
- **External-plugin authority boundary**: user-approved sandbox/object-capability grants for host-provided APIs and handles such as filesystem, network, secrets, model-visible durable notification, Pod-management façade, persistent/plugin state, and authority-bearing service access.

The goal is not to remove the authority model. The goal is to prevent the authority model from being mixed into every internal module extraction.

## Requirements

- Audit `pod::feature` names and APIs for places where external-plugin authority concepts are presented as required for internal built-in modules.
- Make the API clearly support a contribution-only built-in module path.
  - Internal modules should be able to declare contributions and install without requesting authorities.
  - Descriptor/contribution reconciliation still applies.
  - Normal per-call tool permission / PreToolCall policy still applies.
- Keep `HostAuthority` or equivalent only for host-provided dangerous authorities.
  - Do not reintroduce contribution-authority variants such as `ContributeTool`, `ContributeHook`, `RunBackgroundTask`, or `ProvideService`.
- Consider whether the authority request/grant types should be renamed, nested, moved, or documented as external-plugin/sandbox-oriented.
- Clarify how built-in modules receive in-process state/handles from the host without treating those constructor arguments as plugin authorities.
- Clarify how future external plugins will get only approved host handles/services, without changing the internal module path.
- Update tests or add focused tests where needed so a built-in module with zero requested authorities is a normal first-class path.

## Non-goals

- Implementing external plugin loading or package approval lock files.
- Implementing a real user approval resolver.
- Changing existing manifest/tool permission policy.
- Extracting Memory or Pod management out of core.
- Changing Task tool behavior.

## Acceptance criteria

- The code/design clearly distinguishes contribution registration from external-plugin host authority grants.
- Internal built-in modules can be represented as contribution-only modules without authority boilerplate.
- Authority model remains available for future external plugins and host-provided dangerous APIs.
- Tests/documentation make it hard to confuse descriptor-approved contributions with sandbox authorities.
