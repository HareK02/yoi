---
title: 'Plugin: define runtime, surface, and minimal host API model'
state: 'closed'
created_at: '2026-05-31T01:00:05Z'
updated_at: '2026-06-19T13:29:26Z'
assignee: null
---

## Goal

Design Yoi's Plugin model as a user-facing package/config/runtime layer built on top of `pod::feature` and a deliberately small host API.

Use three stable terms:

- **Plugin runtime**: how Plugin code/config is executed.
- **Plugin surface**: what the Plugin adds to Yoi.
- **Plugin host API**: what Yoi-provided capabilities a Plugin runtime may call while implementing a surface.

`pod::feature` remains the internal substrate for contribution registration, lifecycle diagnostics, and Worker/ToolRegistry/HookRegistry integration. Plugin owns package identity, enablement, runtime selection, Plugin-layer permissions, trust policy, and host API grants.

MCP is not the Plugin model and should not be treated as a Plugin permission backend. MCP is a separate protocol-backed integration layer that also uses `pod::feature` and owns MCP-specific enablement/trust policy.

## Background

Yoi already has internal extension-like surfaces:

- Tools via `llm_worker::ToolRegistry` / `ToolDefinition`
- Hooks via `HookRegistry` / `HookEvent` / `HookAction`
- Feature contribution declarations in `pod::feature`
- Built-in features such as task and ticket tools

Plugin design must not let external packages bypass Worker history, prompt/context invariants, scoped tool permissions, restore snapshot authority, or launch-policy authority.

Prior Hook hardening work constrains model-visible context mutation so Plugin exposure can stay safe. Plugin design should reuse those safe surfaces rather than creating parallel prompt/history/tool paths.

A concrete future use case is a chat bridge such as Slack/Discord. The preferred architecture is not that Yoi starts or bundles a full Slack/Discord SDK process. An externally managed bridge service, such as a `discord.js` service, may expose a small HTTPS API; a Yoi Plugin can call that API using configured URL/auth data. A Plugin may also run a Pod-lifetime service loop and submit external events through a host-mediated Ingress surface, but those semantics must stay concrete and bounded.

## Plugin runtime

Runtime answers: **how is Plugin code/config executed?**

Supported design vocabulary:

- `declarative`: config-only Plugin behavior, no arbitrary code execution.
- `wasm`: sandboxed WASM Plugin module with explicit host APIs.
- `external_process`: future/optional runtime. Do not assume it for the initial Plugin model.

The initial runtime direction is:

```text
runtime: declarative and/or wasm
host APIs: https + fs, plus surface-intrinsic ingress/diagnostics where required
surfaces: Tool + Hook + Service + Ingress
```

## Plugin surface

Surface answers: **what does the Plugin add to Yoi?**

The Plugin surfaces are:

- `Tool`
- `Hook`
- `Service`
- `Ingress`

Do not use `Outbound` as a separate surface. External writes are represented as Tool surface entries with side-effect metadata.

### Tool surface

A Tool surface adds model/workflow-callable operations through ordinary Yoi ToolRegistry paths.

External side effects such as chat sends are Tools:

```text
discord_send_message
slack_reply_thread
github_create_issue_comment
```

Requirements:

- registers through ordinary ToolRegistry / `pod::feature` paths
- uses normal model-visible schema exposure
- passes normal PreToolCall permission policy
- commits tool call/result through normal history plumbing
- uses bounded result serialization
- declares side-effect metadata such as `effect = "external_write"` where applicable
- validates configured destination allowlists before executing external writes

### Hook surface

A Hook surface reacts to supported Yoi lifecycle events using the hardened public Hook API.

Requirements:

- no raw hidden `Item` injection
- no prompt/context mutation outside durable history-aware paths
- explicit supported `HookAction` subset only
- Hook-triggered external writes must still go through Tool or another explicit host-approved path, not hidden side effects

### Service surface

A Service surface is Pod-lifetime Plugin work managed by Yoi's Plugin runtime.

The initial Service contract is concrete and limited:

- Services start during Pod startup after explicit Plugin enablement is resolved.
- Services stop when the Pod stops or when the Plugin instance is replaced during restore/relaunch.
- Services report lifecycle state: `starting`, `ready`, `degraded`, `failed`, `stopping`, `stopped`.
- Services may use only granted Plugin host APIs, initially `https` and `fs`, plus surface-intrinsic diagnostics and ingress submission where granted.
- Services must be cancellable and must not block Pod shutdown indefinitely.
- Service diagnostics must be bounded and must redact secrets.

For chat bridge Plugins, Service is the WASM-side client loop that communicates with an externally managed bridge service over HTTPS polling or other explicitly designed future event mechanisms. WebSocket/SSE are not implied by the word Service; they require separate host API design.

### Ingress surface

Ingress is the host-mediated surface for external events entering Yoi.

Plugins do **not** directly choose `Notify`, `Run`, target Pod, or hidden context insertion. A Plugin submits a typed external event to the host:

```text
host.ingress.submit(event)
```

Then host routing policy decides delivery:

```text
submit external event -> route/dedup/bounds/policy -> notify | run | drop | diagnostic
```

Initial typed event target for chat bridges:

```text
ExternalMessageEvent {
  source_plugin,
  provider,
  external_workspace_id/team_id/guild_id,
  channel_id,
  thread_id?,
  message_id,
  actor_id,
  actor_display_name?,
  text,
  attachments[],
  timestamp,
  permalink?,
  dedup_key,
}
```

Requirements:

- external content is untrusted
- event size is bounded before entering Yoi history/model-visible context
- host performs deduplication and loop prevention
- host routing config decides target Pod and delivery mode
- accepted events are appended through durable history/notification paths before they can affect model context
- rejected/dropped events produce bounded diagnostics without unnecessarily storing message content

## Plugin host APIs

Host API answers: **what Yoi-provided capabilities can a Plugin runtime call while implementing a surface?**

Host APIs are runtime capabilities controlled by Plugin-layer grants. Keep this set narrow. Do not add a broad API to avoid deciding concrete semantics.

The initial general-purpose host API set is:

- `https`: bounded HTTPS client only.
- `fs`: scoped filesystem read/write under explicit Plugin grants.

Surface-intrinsic host calls also exist where required:

- `ingress.submit`: available only to Plugins granted the Ingress surface.
- `diagnostics`: bounded lifecycle/error/health reporting for Plugin runtime and Service surface.

Use `https`, not `web`, for the initial network API. The first network host API should not imply arbitrary browser-like web capabilities, raw TCP, local network access, HTTP without TLS, WebSocket, SSE, DNS policy, or remote fetching semantics beyond the explicitly designed HTTPS client.

### `https` host API

Initial requirements:

- HTTPS only
- explicit allowlist of origins or exact endpoints
- bounded request/response size
- timeout/cancellation
- configured headers/auth data only through Plugin enablement policy
- no implicit access to ambient environment variables or host credentials
- diagnostics must redact credentials and sensitive headers

WebSocket, SSE, long polling helpers, and inbound server/webhook APIs are out of the initial host API unless a later Ticket designs them explicitly.

### `fs` host API

Initial requirements:

- scoped paths explicitly granted by Plugin-layer policy
- no automatic inheritance from Pod workspace scope
- bounded read/write operations
- path traversal protection
- diagnostics that identify denied paths without leaking file contents

## Requirements

- Define Plugin as a user-facing layer over `pod::feature`, not as the feature API itself.
  - Plugin package/config/runtime code eventually contributes through `pod::feature`.
  - `pod::feature` remains responsible for contribution declarations, lifecycle, diagnostics, and registration plumbing.
  - Plugin remains responsible for package enablement, provenance, runtime selection, host API grants, and Plugin-layer permission/trust policy.
- Use the terms `Plugin runtime`, `Plugin surface`, and `Plugin host API`; avoid durable/user-facing use of `contribution category`.
- Plugin surfaces are Tool / Hook / Service / Ingress.
- Service has a Pod-lifetime lifecycle contract; it does not imply WebSocket/SSE/raw network support.
- Ingress is `host.ingress.submit(typed external event)`; host routing decides notify/run/drop/diagnostic.
- External outbound side effects are Tool surface entries with external-write metadata, not a separate Outbound surface.
- Initial general-purpose Plugin host APIs are only `https` and `fs`.
- `ingress.submit` and `diagnostics` are surface-intrinsic host calls, not broad ambient capabilities.
- Do not expose a generic `web` API in the initial Plugin model.
- Define Plugin-layer permission and trust model separately from `pod::feature`.
  - Do not rely on feature-level `HostAuthority` / authority grants for Plugin permissions.
  - Plugin grants are tied to package identity/runtime/user or workspace config, not feature install reports.
  - Plugin policy decides which surfaces and host APIs a Plugin runtime can use.
- Define runtime families separately.
  - Declarative/config-only Plugins
  - WASM Plugins
  - External process Plugins only as an explicit future/optional runtime
  - MCP remains a separate feature-backed protocol integration, not the Plugin permission model.
- Plugin metadata, config, external data, and package content are untrusted input.
- Plugin enablement must be explicit.
  - package presence/discovery alone cannot enable or execute Plugin code
  - workspace/user config must opt in to package/runtime/surface activation and host API grants
- The design must document the boundary among Plugin, MCP, and built-in Features.

## Acceptance criteria

- A design note or Ticket plan defines Plugin as a user-facing layer over `pod::feature`.
- The plan uses `runtime`, `surface`, and `host API` terminology and avoids relying on `contribution category` as the durable term.
- The plan states that Plugin surfaces are Tool / Hook / Service / Ingress.
- The plan states that Service has concrete Pod-lifetime lifecycle semantics and does not imply unspecified network/event APIs.
- The plan states that Ingress is typed external-event submission to the host, not direct Notify/Run/context injection.
- The plan states that external outbound side effects are Tool surface entries with external-write metadata.
- The plan states that initial general-purpose Plugin host APIs are `https` and `fs` only.
- The plan states that `https` is HTTPS-only and does not imply generic `web`/browser/raw-network capabilities.
- The plan states that filesystem access is a Plugin host API controlled by Plugin-layer grants and does not inherit Pod workspace scope automatically.
- The plan states that Plugin permissions are Plugin-layer policy and are not implemented by `pod::feature` `HostAuthority` grants.
- The plan states that MCP is a separate feature-backed integration with MCP-specific enablement/trust policy.
- The public Hook safety invariants from `00001KT6Q08R8` remain intact.
- Tool Plugins are required to use ordinary ToolRegistry / permission / history / bounded-result paths.
- No design path allows Plugin package presence alone to execute code or mutate context.
- The design identifies which follow-up Tickets own package format, runtime implementation, host APIs, and Plugin permission details.

## Suggested decomposition

1. Public Hook surface hardening. Completed by `00001KT6Q08R8`.
2. Feature contribution API substrate. Completed by `00001KT6Q08R9` and `00001KTR81P9X`.
3. Plugin package/discovery format. Tracked by `00001KT0Z4BK8`.
4. Plugin enablement plan + metadata snapshot restore integration.
5. WASM/declarative runtime slice with Tool and Hook surfaces.
6. Plugin host API slice for `https` and `fs` only.
7. Service lifecycle surface.
8. Ingress external-message submission and routing policy.
9. Tool external-write metadata and destination allowlist enforcement.
10. Future Ticket: streaming event APIs such as SSE/WebSocket/long polling, if HTTPS polling is insufficient.
11. MCP local stdio integration remains tracked separately by `00001KTR82RB7`.

## Notes

- This Ticket is a design coordination Ticket; implementation should be split into concrete Tickets rather than landing a broad Plugin platform in one change.
- Keep Plugin permission terms distinct from removed feature-layer `HostAuthority` terminology.
- Avoid requiring Yoi to manage arbitrary external SDK processes for chat bridge support.
- Avoid broad host APIs. Add host APIs only when a concrete Plugin surface and use case require them.
