<!-- event: create author: "yoi ticket" at: 2026-07-02T07:02:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T07:03:11Z -->

## Plan

Workspace 初期化を明示 init command に切り出し、serve の初期化副作用をなくす Ticket として詳細化した。

決定:
- `yoi workspace init [--workspace <PATH>]` と `yoi-workspace-server init [--workspace <PATH>]` を追加する。
- init は `.yoi/workspace.toml` と `.yoi/workspace-backend.default.toml` だけを作る。
- init は `.local` config、DB、embedded Runtime fs-store、logs、Worker data を作らない。
- serve は `.yoi/workspace.toml` を load-only し、無ければ `workspace init` を促す typed diagnostic で失敗する。
- serve は default config template を作らない。
- existing legacy serve flags の全面削除は対象外だが、新規 Backend 設定項目を CLI flag として増やさない方針は維持する。


---

<!-- event: decision author: hare at: 2026-07-02T08:31:22Z -->

## Decision

Workspace init Ticket に storage / 正本境界を追記した。

決定:
- 現状の Workspace 初期化は local filesystem marker を作る実装でよい。
- ただし `.yoi` filesystem layout を Workspace の public API contract として固定しない。
- `.yoi/workspace.toml` は local workspace identity marker であり、raw path を Browser-facing API / Runtime create API / Worker conversation context の正本識別子として出さない。
- Backend DB と embedded Runtime fs-store は generated local data であり、Workspace / Project record の正本ではない。
- Ticket / Objective など project record は現状 local filesystem backend を正本としてよいが、init は provider-specific project record layout を生成しない。
- 将来 ProjectRecordBackend / TicketBackend / ObjectiveBackend 相当の provider を差し替える余地を残す。


---

<!-- event: intake_summary author: hare at: 2026-07-02T09:03:56Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T09:03:56Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T09:03:56Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T09:03:56Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-02T09:20:10Z -->

## Implementation report

Implemented explicit Workspace init and removed serve-time initialization side effects.

Changes:
- Added `yoi workspace init [--workspace <PATH>]` passthrough to `yoi-workspace-server init`.
- Added `yoi-workspace-server init [--workspace <PATH>]`.
- `init` creates only `.yoi/workspace.toml` and `.yoi/workspace-backend.default.toml`.
- `init` does not create `.local` config, SQLite DB, embedded Runtime store, logs, Worker data, or project record layout.
- Added `WorkspaceIdentity::load_required(...)` for serve-time load-only behavior.
- `serve` now requires an initialized workspace and no longer calls `load_or_init` or copies the default config template.
- Uninitialized `serve` fails with a diagnostic telling the user to run `yoi workspace init --workspace <path>`.
- Updated help text for `yoi workspace` and `yoi-workspace-server`.
- Added focused tests for init parsing, init output files, load_required no-create behavior, and yoi CLI passthrough.

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo test -p yoi`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- manual smoke: uninitialized `yoi-workspace-server serve` fails without creating identity.
- manual smoke: `yoi-workspace-server init` creates only workspace identity and default template.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T09:20:10Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-02T09:20:26Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-02T09:20:26Z status: closed -->

## 完了

Implemented explicit Workspace initialization command and removed implicit initialization from serve.

Delivered:
- `yoi workspace init [--workspace <PATH>]`.
- `yoi-workspace-server init [--workspace <PATH>]`.
- `init` creates `.yoi/workspace.toml` and `.yoi/workspace-backend.default.toml` only.
- `serve` requires `.yoi/workspace.toml` to already exist and fails with an init diagnostic otherwise.
- `serve` no longer creates workspace identity or default config template.
- Help text and focused tests updated.

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo test -p yoi`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`


---
