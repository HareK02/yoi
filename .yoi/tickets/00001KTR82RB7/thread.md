<!-- event: create author: intake at: 2026-06-10T07:48:49Z -->

## 作成

LocalTicketBackend によって作成されました。

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

<!-- event: state_changed author: hare at: 2026-06-20T05:33:15Z from: planning to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-20T05:33:15Z status: closed -->

## 完了

Closed as superseded by concrete MCP implementation Tickets.

This Ticket bundled config/trust policy, stdio lifecycle, tools/list registration, tools/call execution, resources/prompts operations, result serialization, and list_changed handling into one broad implementation item. That is too coarse for the current Ticket policy: Tickets should be concrete implementation tasks.

The MCP roadmap now lives in Objective `00001KTR80WMN` (`MCP local stdio integration roadmap`). Concrete follow-up Tickets are:
- `00001KVHR3WRF` — local stdio server config and trust policy;
- `00001KVHR3WRY` — stdio JSON-RPC lifecycle client;
- `00001KVHR3WS6` — server tools registration into ToolRegistry;
- `00001KVHR3WSD` — tools/call execution through ordinary Tool path;
- `00001KVHR3WSN` — resources/prompts as explicit tool operations;
- `00001KVHR3WSW` — list_changed notification handling.

Future MCP work should use those concrete Tickets or similarly scoped follow-ups, not this broad umbrella Ticket.


---
