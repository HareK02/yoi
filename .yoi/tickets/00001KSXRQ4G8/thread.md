<!-- event: create author: tickets.sh at: 2026-05-31T01:00:05Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-31T01:01:09Z -->

## Decision

Initial decision note from user discussion:

- The plugin surface should mainly expose Hooks and Tools.
- MCP is related, but should be treated as one protocol-bound backend/bridge rather than the entire plugin model.
- Planned plugin mechanisms:
  - MCP for protocol-constrained external capability providers;
  - TOML/config-only hooks for simple behavior that does not need arbitrary code;
  - WASM for programmable Hooks/Tools with explicit host capabilities.
- General scripting languages were considered, but the initial direction is WASM because it offers a clearer sandbox/capability boundary.


---

<!-- event: decision author: hare at: 2026-06-03T12:25:05Z -->

## Decision

# Decision: split feature registry and Hook hardening from Plugin architecture

The Plugin architecture ticket remains the broad architecture surface for Tools, Hooks, runtime kinds, capability model, trust model, discovery/enablement, and MCP/WASM/declarative runtime mapping.

Two implementation-oriented prerequisite tickets are split out:

- `plugin-feature-contribution-registry`: define and implement the Pod-layer feature contribution registry so built-in and future external capabilities register through existing Tool / Hook / Notify paths instead of ad hoc Pod code paths.
- `hook-public-surface-hardening`: audit and harden `pod::hook` before exposing it as a feature/plugin contribution boundary, especially removing public access to raw internal action types that can inject model-visible `Item` values.

This preserves the desired detachable shape: feature state remains in the feature/extension module, while Pod interaction happens through existing durable host surfaces. WorkItem management should be implemented as a built-in feature contribution once the registry boundary is in place, rather than as a special Pod context-injection path.


---

<!-- event: decision author: hare at: 2026-06-13T15:29:21Z -->

## Decision

決定:
- `pod::feature` は API / contribution substrate として扱い、Plugin や MCP の権限管理を担わせない。
- Plugin は `pod::feature` をユーザー向け package/config/runtime 形式で使わせる層であり、Plugin permission / trust policy は Plugin layer で定義する。
- MCP は `pod::feature` 上に protocol-backed integration layer を構築するが、MCP server enablement / command-env-secret policy / trust boundary / MCP-specific permission は MCP layer が独自に持つ。
- MCP local stdio server の OS-level side effects は Yoi feature authority では制御できないため、feature-layer authority / grant を MCP や Plugin の permission model に流用しない。

反映:
- `00001KTR81P9X` は authority ではなく provider lifecycle / dynamic contribution / normal ToolRegistry path / untrusted normalization に絞る。
- `00001KTR82RB7` は MCP 固有の explicit config と trust model を持つ。
- `00001KSXRQ4G8` と `00001KT0Z4BK8` は Plugin permission を Plugin layer として扱い、MCP を初期 Plugin packaging/runtime から分離する。


---
