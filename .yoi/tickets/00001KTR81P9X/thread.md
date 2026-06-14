<!-- event: create author: intake at: 2026-06-10T07:48:14Z -->

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

<!-- event: intake_summary author: hare at: 2026-06-13T16:27:15Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T16:27:15Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T06:08:25Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-14T06:10:02Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket is queued and requirements/acceptance criteria for feature-layer provider/service lifecycle and dynamic contributions are specified.
- Outgoing `depends_on 00001KV0SP0TY` is satisfied: HostAuthority cleanup is done and merged.
- Relation/orchestration plan checks show no remaining blocker.
- Scope is API/contribution substrate only; Plugin/MCP permission/trust policy remains out of scope.

IntentPacket:
- Extend `pod::feature` so protocol-backed providers can contribute startup-discovered tools/services/background lifecycle diagnostics through ordinary ToolRegistry / Worker / permission / history / bounded-result paths.

Binding invariants:
- Do not introduce `HostAuthority` or replacement feature-layer authority grants.
- Do not implement MCP protocol/trust policy or Plugin permission policy here.
- Dynamic schemas must be stable for a run; provider metadata/output is untrusted; tool calls must go through normal permission/history/result paths.

Validation:
- focused feature/provider tests, affected crate tests, `cargo fmt --check`, `git diff --check`, `cargo check --workspace --all-targets`, and `nix build .#yoi` if feasible.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T06:10:45Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence, satisfied dependency on HostAuthority cleanup, relation records, orchestration plan, and clean workspace state were checked. No blockers remain; accept for implementation before worktree/spawn side effects.

---
