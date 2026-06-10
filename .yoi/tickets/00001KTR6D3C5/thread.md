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
