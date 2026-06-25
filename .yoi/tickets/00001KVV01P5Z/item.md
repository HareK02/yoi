---
title: 'Planning Ticket API and UI without queue operations'
state: 'planning'
created_at: '2026-06-23T19:41:51Z'
updated_at: '2026-06-25T16:38:49Z'
assignee: null
---

## 背景

Ticket 管理 UI を作るにあたり、最初から DB migration や Queue 操作まで含めると authority 境界が大きくなりすぎる。現行の Ticket authority は `.yoi/tickets/<ticket-id>/` の flat file backend であり、Workspace server 側にも Ticket 用 SQLite schema の器はあるが、現時点では authority ではない。

この Ticket では migration を行わず、まず Workspace backend に Ticket 操作用 API を整備し、その API を操作する最小 UI を実装する。対象は Planning Ticket の作成・確認に限定し、`ready -> queued` の Queue 操作や Orchestrator 起動には踏み込まない。

## 目的

- Workspace web から Ticket を確認し、Planning 状態の Ticket を作成できるようにする。
- UI は backend API を通じて Ticket を操作し、frontend が `.yoi/tickets` の内部ファイル構造や authority path に依存しないようにする。
- 将来 DB authority / migration に進む余地を残しつつ、この Ticket では現行 file backend を正として扱う。

## 要件

### Backend API

- Workspace server に Ticket 管理用 API を追加または整理する。
- v0 では現行 `LocalTicketBackend` / project record reader を authority として使い、DB migration はしない。
- API は少なくとも以下を扱う。
  - Ticket 一覧取得
  - Ticket 詳細取得
  - Planning 状態の Ticket 作成
- 作成 API は title と本文/背景/受け入れ条件などの必要最小限の入力を受け、canonical Ticket ID を返す。
- frontend から raw `.yoi/tickets` path や内部 artifact path を直接指定させない。
- Ticket ID は canonical ID のみを扱い、title slug や legacy alias を API contract にしない。
- API の error は typed response として扱う。
  - invalid input
  - duplicate / conflict
  - backend unavailable
  - workspace not found / outside current workspace

### UI

- Workspace web に Ticket 管理画面または Ticket 作成導線を追加する。
- v0 UI は Planning Ticket の作成までに限定する。
- 作成フォームは最低限以下を扱う。
  - title
  - 背景 / 要件の本文
  - 受け入れ条件
- 作成後は作成された canonical Ticket ID と状態を表示し、詳細画面または一覧へ反映する。
- UI は API response を authority とし、frontend 側で Ticket file layout を再実装しない。

### Scope boundary

- Queue 操作は実装しない。
  - `ready -> queued`
  - Panel / Workspace UI からの Queue button
  - Orchestrator / Coder / Reviewer 起動
  - role-session claim 作成
- Ticket DB migration は実装しない。
- 既存 `.yoi/tickets` authority を変更しない。
- 既存 Ticket CLI / Pod tool の挙動を壊さない。

### Safety / authority

- Browser は local path / runtime path / socket path を直接 authority として渡さない。
- Workspace backend が current workspace の Ticket backend root を解決する。
- Ticket 作成は current workspace の `.yoi/tickets` に限定する。
- 将来 permission model を挟めるよう、API handler と backend operation の境界を分ける。

## Non-goals

- Ticket storage の DB migration。
- SQLite `tickets` schema を authority に昇格すること。
- Queue / ready / inprogress / close などの lifecycle mutation UI。
- Orchestrator 起動、Worker spawn、role-session claim 連携。
- Ticket relation / artifact / orchestration-plan 編集 UI。
- SSE/WebSocket による live Ticket update。

## 受け入れ条件

- Workspace server に Ticket list/detail/create planning 用 API がある。
- Planning Ticket 作成 API が canonical Ticket ID を返す。
- 作成された Ticket は既存 `yoi ticket list/show` から確認できる。
- Workspace web から Planning Ticket を作成できる。
- UI から Queue 操作はできない。
- DB migration や SQLite Ticket authority 化を行っていない。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task build` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
