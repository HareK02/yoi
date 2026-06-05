# Decision: add host-mediated Feature services

Add Service provider/consumer support to the Plugin/Feature base Pod API.

This is not a decision to extract currently implemented core features such as Memory or Pod management immediately. Existing implementations may remain core-backed. The new service form exists so future built-in features and plugins can expose stable APIs to other features without direct concrete dependencies or ad hoc Pod internals access.

Revised contribution/dependency model:

- Contributions:
  - ToolContribution
  - HookContribution
  - BackgroundTaskContribution
  - ServiceProvider / ServiceDeclaration
- Dependencies:
  - ServiceRequirement, resolved by the host registry before feature installation

Rules:

- A feature/plugin may provide a public service through a host-owned service registry.
- Another feature/plugin may acquire that service only through the host, after dependency resolution and capability grant checks.
- Consumers do not import provider concrete types, private state, raw process handles, raw WASM/MCP handles, or plugin-specific modules.
- Required missing services skip the consuming feature with diagnostics; optional missing services allow degraded installation when supported.
- Service cycles are rejected initially.
- In-process built-ins may use Rust trait-object handles internally, but the public design must leave room for external plugin service proxies.
- Service handles must be capability-bound so acquiring a broad service does not become an authority escalation path.

Examples:

- `builtin:memory` may provide `yoi.memory.v1`; other features can optionally consume read-only memory lookup without depending on Memory internals.
- `builtin:pod-orchestration` may provide `yoi.pod-management.v1` as a controlled façade while the actual Pod lifecycle/scope authority remains host-owned.
- Future issue-tracker plugins may provide `project.issue-tracker.v1` for WorkItem integration.
