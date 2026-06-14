---
title: 'Extend pod::feature API for external protocol-backed capability providers'
state: 'done'
created_at: '2026-06-10T07:48:14Z'
updated_at: '2026-06-14T06:39:48Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['feature-api', 'tool-registry', 'permission-scope', 'prompt-context', 'dynamic-registry', 'service-lifecycle']
queued_by: 'workspace-panel'
queued_at: '2026-06-14T06:08:25Z'
---

## Background

Yoi needs `pod::feature` to serve as the common API substrate for capabilities that contribute Tools / Hooks / Services / BackgroundTasks. External protocol-backed providers such as MCP need to discover contributions at startup and expose them through the same Worker / ToolRegistry / permission / history / bounded-result paths as built-in tools.

This Ticket defines the feature-layer API required for those providers. It does **not** define Plugin package permissions, MCP server trust policy, local process sandboxing, or a feature-level authority model. Plugin and MCP are separate layers that build on the feature API and own their own user-facing configuration and permission/trust decisions.

Objective context: `00001KTR80WMN`.

## Requirements

- Treat `pod::feature` as the API layer for contribution registration and lifecycle integration.
  - Feature code exposes contribution types and installation/runtime hooks.
  - Feature code does not decide Plugin trust, MCP enablement, or external-provider permissions.
  - Do not add or rely on `HostAuthority` / authority grants for MCP, Plugin, or provider-process permissions.
- Add a feature-owned provider/service lifecycle surface sufficient for protocol-backed providers.
  - startup / ready / failed / stopped diagnostics
  - Pod lifetime integration and deterministic shutdown hooks
  - provider failure state that makes dependent dynamic tools unavailable with a clear diagnostic
- Add dynamic contribution registration for startup-discovered tools.
  - discovery happens before the tool schema is exposed to a model request
  - registered tools use stable namespacing and normal ToolRegistry registration
  - tool schema changes during an active run do not silently mutate the model-visible schema
- Define bounded refresh semantics for provider list changes.
  - initial implementation may defer live refresh to a turn boundary or report restart/reinitialize-required diagnostics
  - silent stale or mid-run schema mutation is not allowed
- Define provider metadata / schema / result normalization as untrusted data.
  - descriptions, schemas, annotations, titles, and provider metadata cannot weaken system/developer instructions
  - structured / rich results are converted through a bounded Yoi representation before entering history/model-visible output
- Ensure dynamic tool calls do not bypass normal Yoi paths.
  - PreToolCall permission hooks still run
  - permission denial prevents the provider call
  - tool results and errors are committed through normal history/result plumbing
- Keep MCP-specific protocol details in Ticket `00001KTR82RB7`.
  - command / args / cwd / env / stdio JSON-RPC / MCP lifecycle are MCP-layer concerns
  - MCP may define its own enablement and trust policy on top of this feature API
- Keep Plugin user-facing format and Plugin permission model outside this Ticket.
  - Plugin packages/configs may use the feature API to contribute capabilities
  - Plugin permission/trust policy belongs to the Plugin layer, not `pod::feature`

## Acceptance criteria

- `pod::feature` can represent a provider/service that becomes ready, fails, and stops with install/runtime diagnostics visible to the host.
- A mock protocol-backed provider can register at least one startup-discovered dynamic tool before the first model request that would expose its schema.
- The dynamic tool is registered as an ordinary Yoi tool and is visible through the same model-visible schema path as built-in tools.
- A PreToolCall permission denial for the dynamic tool prevents the mock provider from receiving a call.
- Provider failure makes dependent dynamic tools unavailable with a bounded, comprehensible diagnostic rather than panic or stale silent failure.
- Oversized or structured provider result data is bounded before being committed as a tool result.
- A simulated list-changed/schema-change event is handled by a documented safe path: turn-boundary refresh, restart/reinitialize-required diagnostic, or explicit unsupported diagnostic.
- Tests/docs make clear that `pod::feature` is not the Plugin permission layer and not the MCP server trust/sandbox layer.
- No new `HostAuthority` / authority-grant dependency is introduced for external providers, Plugin permissions, or MCP local stdio execution.
- Validation: focused feature/provider tests, affected crate tests, `cargo fmt --check`, `cargo check --workspace --all-targets`, and `nix build .#yoi`.

## Binding decisions / invariants

- `pod::feature` is an API/contribution substrate, not a security or trust-policy layer.
- Plugin is a user-facing package/config/runtime layer that uses `pod::feature`; Plugin permissions are Plugin-layer policy.
- MCP is a separate feature-backed integration layer; MCP enablement, local server trust, and MCP-specific permissions are MCP-layer policy.
- Dynamic provider contributions must enter through ordinary Worker / ToolRegistry / permission / history / bounded-result paths.
- Model-visible tool schemas are stable for the duration of a model request/run.
- External provider metadata and output are untrusted content.

## Out of scope

- MCP protocol implementation and local stdio transport details.
- Plugin package format, distribution, provenance, and Plugin runtime permission grants.
- OS-level sandboxing of external provider processes.
- Removing all existing `HostAuthority` code from the repository; cleanup is tracked separately by `00001KV0SP0TY` unless directly necessary for this API slice.

## Escalation conditions

- The feature API appears to need policy decisions about Plugin trust or MCP server permissions.
- Dynamic registry refresh would require mutating model-visible tool schema during an active run.
- Provider result normalization would require provider-native multimodal blocks to bypass normal bounded tool-result serialization.
- MCP implementation pressure suggests adding private ToolRegistry/history/context bypasses instead of extending the feature API.

## Related work

- Objective: `00001KTR80WMN`
- MCP implementation: `00001KTR82RB7`
- Plugin extension surface: `00001KSXRQ4G8`
- Plugin package/discovery: `00001KT0Z4BK8`
- Feature authority cleanup: `00001KV0SP0TY`
- Decomposed from broad Ticket: `00001KST8H4M0`
