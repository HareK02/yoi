<!-- event: create author: "yoi ticket" at: 2026-07-01T14:41:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-01T14:43:04Z -->

## Plan

Workspace Backend config file schema を定義する Ticket として詳細化した。

決定:
- 新規 Backend 設定項目は CLI flag として増やさない。
- config file は固定 path `<workspace_root>/.yoi/workspace-backend.local.toml` から読む。
- config file は workspace-local だが `.local` 名にして git 管理外にする。
- config は server/listen/static assets/data path override/limits/remote runtime source などの方針だけを持つ。
- data 本体、workspace identity、Ticket/Objective、Runtime fs-store、transcript、secret 値は config file に入れない。


---

<!-- event: intake_summary author: hare at: 2026-07-01T15:32:38Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-01T15:32:38Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-01T15:32:38Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-01T15:32:39Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-01T15:48:09Z -->

## Implementation report

Implemented Workspace Backend config file schema and resolver.

Changes:
- Added `workspace_server::config` with `WorkspaceBackendConfigFile` serde TOML schema.
- Fixed config path: `<workspace_root>/.yoi/workspace-backend.local.toml`.
- Config file absence resolves to defaults.
- Unknown TOML fields are rejected via `deny_unknown_fields`.
- Relative paths resolve from workspace root; absolute paths are preserved.
- Added resolved config boundary for `ServerConfig`, listen address, and database path.
- Added data root derivation for `workspace.db` and `embedded-runtime`.
- Added remote runtime config schema with `token_ref`, while failing closed until secret ref resolution exists; raw token values are not in schema.
- Updated `yoi-workspace-server serve` to load config before applying existing legacy dev CLI overrides.
- Added focused tests for defaults, unknown fields, path resolution, data root derivation, secret value rejection, and token_ref fail-closed behavior.

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-01T15:48:17Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
