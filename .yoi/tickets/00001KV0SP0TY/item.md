---
title: 'Remove feature-layer HostAuthority model'
state: 'closed'
created_at: '2026-06-13T15:30:22Z'
updated_at: '2026-06-14T14:00:13Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['feature-api', 'tool-registry', 'ticket-tools']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T16:33:15Z'
---

## Background

Current `pod::feature` contains `HostAuthority`, `HostAuthorityRequest`, and `HostAuthorityGrantSet`. In practice this layer does not enforce OS-level or Plugin/MCP permissions: current install code grants requested authorities and uses them mostly as declaration/reporting and contribution registration checks.

The project decision is to keep `pod::feature` as an API/contribution substrate and move real permission/trust decisions to the owning layers:

- Plugin package/runtime permission policy belongs to the Plugin layer.
- MCP local stdio enablement, command/env/secret policy, and server trust belong to the MCP layer.
- filesystem/tool/pod permissions remain in existing manifest/profile/tool permission paths.
- Ticket feature access remains explicit Ticket feature configuration and backend validation, not feature-layer authority grants.

Therefore the feature-layer authority model should be removed, not renamed or preserved as diagnostics. Any remaining report data should use ordinary contribution/installation diagnostic terminology without `Authority`, `Grant`, or permission semantics.

## Requirements

- Remove `HostAuthority`, `HostAuthorityRequest`, `HostAuthorityGrantSet`, and related feature install report fields from `pod::feature`.
- Do not introduce renamed feature-layer authority/grant types as a replacement; use ordinary diagnostics for non-permission install information.
- Remove contribution registration checks that depend on feature-layer authority grants.
- Keep contribution declaration/registration checks that are still useful without authority terminology.
  - duplicate contribution names
  - undeclared contribution categories, if currently checked
  - feature install diagnostics
- Update built-in features that currently request host authorities.
  - Ticket feature should rely on validated `TicketFeatureConfig`, backend root validation, and access-level configuration.
  - Task feature should not mention host authority.
- Ensure no Plugin or MCP Ticket depends on feature-layer authority grants after the cleanup.
- Update comments/docs/tests that describe feature-layer authority as future Plugin/MCP permission infrastructure.
- Do not use this cleanup to introduce Plugin permission policy or MCP server trust policy; those remain separate layers.

## Acceptance criteria

- The codebase no longer exposes `HostAuthority*` or equivalent feature-layer authority/grant types as part of the `pod::feature` public API.
- Built-in task and ticket feature tests pass without authority grants.
- Ticket tools still require explicit Ticket feature config/access and validated backend root.
- Feature install reports still provide useful diagnostics for skipped/failed contributions without implying security grants.
- Grep/review confirms Plugin and MCP planning Tickets do not rely on feature authority for permission/trust control.
- Validation: focused feature/ticket/task tests, `cargo fmt --check`, affected crate tests, `cargo check --workspace --all-targets`, and `nix build .#yoi`.

## Related work

- Feature API dynamic provider work: `00001KTR81P9X`
- MCP local stdio integration: `00001KTR82RB7`
- Plugin extension surface: `00001KSXRQ4G8`
- Plugin package/discovery: `00001KT0Z4BK8`
- Earlier feature registry scaffold: `00001KT6Q08R9`
- Earlier authority naming split: `00001KTAGM2V0`
