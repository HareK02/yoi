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

<!-- event: decision author: hare at: 2026-06-14T16:53:27Z -->

## Decision

決定:
- Durable terminology は `Plugin runtime` / `Plugin surface` / `Plugin host API` にする。`contribution category` は分かりづらいため設計語彙として使わない。
- Plugin surface は Tool / Hook / Service / Ingress の4つに整理する。
- Outbound は独立 surface にしない。Slack/Discord 送信など外部副作用は `external_write` metadata を持つ Tool として扱う。
- Ingress は Plugin が Notify/Run を直接呼ぶ API ではなく、typed external event を host に submit する API とする。host routing policy が notify/run/drop を決める。
- Web/FS/Secret/State/Timer/Diagnostics は Plugin surface ではなく Plugin host API。WASM などの runtime が surface を実装するために、Plugin-layer grant で明示的に許可された範囲だけ使える。
- Chat bridge target は、Yoi が discord.js/Slack SDK process を起動する前提にしない。外部管理の bridge service に WASM Plugin が configured URL + SecretRef で接続する設計を first-class にする。


---

<!-- event: decision author: hare at: 2026-06-14T17:19:59Z -->

## Decision

決定:
- Initial Plugin surface は Tool / Hook を主軸にする。
- Service / Ingress は将来候補として残すが、Submit/Notify/Run や lifecycle semantics を具体化する別 Ticket まで初期 contract には入れない。
- Outbound は独立 surface にしない。外部副作用は `external_write` metadata を持つ Tool として扱う。
- Plugin host API は初期は `https` と `fs` に絞る。`web` という広い API 名は使わず、HTTPS-only の bounded client と scoped FS として設計する。
- WebSocket/SSE/long polling、timer、state、secret plaintext access、Ingress submit API は必要性が具体化した時点で追加設計する。


---

<!-- event: decision author: hare at: 2026-06-14T17:22:23Z -->

## Decision

決定:
- Service / Ingress は Plugin surface として必要なので初期設計対象に戻す。
- Service は Pod-lifetime の Plugin work として具体化する。ただし Service という語は WebSocket/SSE/raw network support を含意しない。初期の外部通信は `https` host API の範囲に限定し、streaming API は別途設計する。
- Ingress は `host.ingress.submit(typed external event)` として具体化する。Plugin が Notify/Run/context injection を直接選ばず、host routing policy が notify/run/drop/diagnostic を決める。
- General-purpose host API は引き続き `https` と `fs` に絞る。`ingress.submit` と `diagnostics` は surface-intrinsic host calls として扱い、広い ambient capability にはしない。


---
