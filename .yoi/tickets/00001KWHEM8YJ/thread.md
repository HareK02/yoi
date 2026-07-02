<!-- event: create author: "yoi ticket" at: 2026-07-02T12:59:57Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T13:00:58Z -->

## Plan

Workspace Browser から手動 Coding Worker を作成する導線として Ticket を詳細化した。

方針:
- Sidebar WORKER heading 横に `New` button を追加する。
- New form は runtime / display name / initial text の最小入力にする。
- Browser は Runtime create request を直接送らず、product-level `/api/workers` endpoint を使う。
- Backend が `kind=coding` を default coding Worker launch に解決する。
- Browser-facing payload に raw path / cwd / ConfigBundleRef / requested capabilities / secret / Runtime endpoint / store path を含めない。
- 成功後は作成 Worker の Console に遷移し、失敗時は sanitized diagnostic を form に表示する。


---

<!-- event: decision author: hare at: 2026-07-02T13:06:46Z -->

## Decision

Manual Coding Worker create form fields を明確化した。

決定:
- v0 form は `display_name`、`runtime_id`、`profile`、`initial_text` の 4 field に限定する。
- `kind = coding` は endpoint / Backend 側 launch mode として扱い、form input や Browser-facing payload field にはしない。
- `profile` は Browser 自由入力ではなく、Backend が公開する候補から選ぶ。
- Browser-facing request は `runtime_id` / `display_name` / `profile` / `initial_text` にし、`ConfigBundleRef`、requested capabilities、raw path、cwd、secret、Runtime endpoint、store path は含めない。


---

<!-- event: intake_summary author: hare at: 2026-07-02T15:39:49Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T15:39:49Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-02T16:13:24Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
