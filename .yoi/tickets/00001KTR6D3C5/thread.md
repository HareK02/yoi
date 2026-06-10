<!-- event: create author: "yoi ticket" at: 2026-06-10T07:19:31Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-10T07:41:20Z -->

## Plan

## Intake refinement

Readiness: `implementation_ready`。

重複確認: `.yoi/tickets` 内で `profile.import` / `import/extend` / global `yoi` に関する未完了の重複 Ticket は見つからなかった。

Open questions: blocking な未決定点はない。`require("yoi")` / `require("yoi.*")` を互換 alias として残すか非推奨にするかは、Ticket 本文どおり実装時に整理し、テストで固定する implementation latitude として扱う。

Risk flags / reviewer focus: `profile-api`, `authority-boundary`, `sandbox-module-loading`, `backward-compat`, `packaging-resource`。

Orchestrator routing: queued 後は implementation に進めてよい。実装では Lua Profile sandbox の global 注入、builtin/raw Profile artifact import、extend merge semantics、validation boundary、bundled `resources/profiles/default.lua` 更新、manifest crate テストを一体で確認すること。

---

<!-- event: intake_summary author: intake at: 2026-06-10T07:41:26Z -->

## Intake summary

既存 Ticket 00001KTR6D3C5 を確認し、Lua Profile の global `yoi` API 注入、`yoi.profile.import/extend`、deep merge semantics、Profile validation boundary、default.lua 移行、local require 維持、manifest crate tests まで受け入れ条件が揃っていると判断した。blocking open question はない。`require("yoi")` / `require("yoi.*")` の扱いは実装時に固定する implementation latitude。主な risk flags は profile-api / authority-boundary / sandbox-module-loading / backward-compat / packaging-resource。

---

<!-- event: state_changed author: intake at: 2026-06-10T07:41:26Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake により implementation_ready と判断したため、state を ready にする。実装開始は panel による ready -> queued と Orchestrator routing に委ねる。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-10T08:04:05Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-10T09:32:26Z from: queued to: inprogress reason: orchestrator_queue_acceptance field: state -->

## State changed

Queue acceptance: relation query has no blockers, existing orchestration plan only has the prior human-gate waiting note, workspace is clean, and user explicitly requested continuing inprogress/queued consumption. Implementation side effects will proceed in a dedicated worktree/branch with review before merge-completion.

---

<!-- event: plan author: orchestrator at: 2026-06-10T09:32:26Z -->

## Plan

Routing decision: implementation_ready

Reason:
- Ticket is `queued`, relation query has no blockers, and existing orchestration plan only contains a prior human-gate waiting note.
- User explicitly requested continuing inprogress/queued work; this satisfies the prior human gate.
- The Ticket contains clear binding requirements, acceptance criteria, non-goals, and reviewer focus.

Evidence checked:
- Ticket body/thread/artifacts.
- `TicketRelationQuery`: no relations/blockers.
- `TicketOrchestrationPlanQuery`: one waiting note from prior target-unspecified routing pass.
- Current workspace state is clean; existing inprogress worktrees use separate branches/scopes.
- Code map target: `crates/manifest/src/profile.rs`, Lua Profile helper/builtin loading paths, `resources/profiles/default.lua`, and manifest crate profile tests.

IntentPacket:
- Intent: make reusable Lua Profiles feel like a small DSL by injecting global `yoi` helpers and adding raw Profile artifact import/extend composition.
- Binding decisions / invariants: Profiles remain Lua-first reusable recipes only; import/extend must not import resolved runtime Manifest or allow runtime-bound/concrete authority fields; final artifact still goes through existing Profile validation; project-local `require("_base")` keeps working; no tombstone/nil deletion semantics in this Ticket; bundled default Profile moves to global `yoi` style.
- Requirements / acceptance criteria: filesystem and embedded Profiles can access global `yoi`; `yoi.profile.import("builtin:default")` returns a raw Profile artifact; `yoi.profile.extend("builtin:default", overrides)` deep-merges nested tables while replacing scalars/lists; validation rejects runtime-bound or concrete authority contamination; require alias handling is fixed by tests; manifest targeted tests pass.
- Implementation latitude: decide whether legacy `require("yoi")` / `require("yoi.*")` aliases remain supported or are deprecated, but fix the behavior in tests; local organization and helper names may vary if the public `yoi.profile.import/extend` surface is preserved.
- Escalate if implementation requires changing Profile authority boundaries, permitting resolved Manifest import, changing runtime startup semantics, or adding deletion/tombstone semantics.
- Validation: `cargo fmt`, `cargo test -p manifest profile --lib` or narrower named tests plus broader manifest tests, `target/debug/yoi ticket doctor`, and `nix build .#yoi` before merge because runtime resources/Profile evaluation/package resources are touched.

---
