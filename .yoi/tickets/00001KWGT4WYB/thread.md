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
