<!-- event: create author: "yoi ticket" at: 2026-07-18T03:04:36Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T03:05:10Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:05:10Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:05:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:05:10Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T03:07:25Z -->

## Implementation report

`builtin:default` profile と setup wizard 生成の `user:default` profile で Ticket feature を有効化した。

変更点:
- `resources/profiles/default.dcdl` の `feature.ticket.enabled` を `true` に変更。
- builtin default profile artifact の `feature.ticket.enabled` を `true` に変更。
- setup wizard が生成する default profile の `[feature.ticket] enabled` を `true` に変更。
- builtin default profile resolution / setup profile generation のテスト期待値を追加。
- `ticket_orchestration` は default では無効のまま維持。
- `WorkerManifestConfig::builtin_defaults()` の裸の feature default は変更していない。

検証:
- `cargo test -p manifest builtin_default_resolves_without_external_evaluator --quiet`
- `cargo test -p manifest builtin_role_profiles_preserve_role_tool_policy --quiet`
- `cargo test -p tui write_default_profile_config_creates_registry_and_profile --quiet`
- `cargo test -p worker installs_ticket_tools_when_default_root_is_usable --quiet`
- `cargo fmt --check`
- `git diff --check`


---

<!-- event: state_changed author: hare at: 2026-07-18T08:32:27Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T08:32:27Z status: closed -->

## 完了

Default profile now enables Ticket lifecycle tools by default while keeping ticket orchestration disabled. Builtin default profile and setup-generated user default profile were updated, with focused tests covering the new expectations.


---
