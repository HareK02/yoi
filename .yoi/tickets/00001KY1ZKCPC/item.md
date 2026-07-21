---
title: 'Backend runtime経由の操作をprotocol transportへ統一しTUI同等にする'
state: 'planning'
created_at: '2026-07-21T09:20:07Z'
updated_at: '2026-07-21T09:47:10Z'
assignee: null
readiness: 'draft'
---
## 背景

Backend runtime worker list / attach 経路が入ったので、次は attach 後の Worker control 操作を TUI 同等に Backend/runtime 経由でも使えるようにする。

現在の local TUI attach は Worker protocol の `Method` を送る操作を持っている。一方、Backend runtime attach client と browser Console は、操作系を ad-hoc な HTTP endpoint に map している。

現状例:

- read/observation: `/events/ws`
- input: `POST /input`
- completions: `POST /completions`
- cancel/stop: 個別 lifecycle endpoint
- pause / resume / rewind: Backend runtime attach では unsupported

この Ticket では、この HTTP 操作系 endpoint の寄せ集めを前提にしない。Backend/runtime 経由でも TUI と同じ `protocol::Method` を送れる protocol transport を作り、TUI Backend attach と browser frontend Console の操作経路をそこへ統一する。

## Target

- 操作系 HTTP endpoint を主経路として使う設計を廃止する。
  - 少なくとも frontend/TUI の新しい操作実装は protocol transport を使う。
  - 既存 endpoint を一時 compatibility layer として残す場合は、Ticket 内で廃止対象として明示し、frontend/TUI からは使わない。
- Backend/runtime に Worker protocol 操作用 transport を実装する。
  - 既存 `/events/ws` が read-only observation なら、双方向 console WS または command WS を追加する。
  - transport payload は既存 `protocol::Method` / `protocol::Event` との対応を保つ。
  - 個別 HTTP API ではなく「protocol method を Backend/runtime に中継する経路」を authority とする。
- TUI Backend Runtime client を protocol transport に寄せる。
  - 現在 `backend_runtime.rs` で unsupported になっている pause / resume / rewind 系を解消する。
  - local TUI attach と同じ `Method` 操作が Backend runtime attach でも使えるようにする。
- browser frontend Console も protocol transport に寄せる。
  - 現在の HTTP `POST /input` / `POST /completions` 依存を廃止対象にする。
  - Console UI から TUI と同等の操作ができるようにする。
  - Worker event observation と command submission が protocol として整合する。

## 必須操作

Backend/runtime 経由で、TUI 同等に以下を使えるようにする。

- run / notify 相当の入力送信
- completion lookup
- cancel
- pause
- resume
- compact（既存 TUI 操作と同等に扱う）
- rewind target list
- rewind to target

## 受け入れ条件

- Backend runtime attach 中の TUI で、local Worker attach と同等の操作が使える。
  - run / notify
  - completion lookup
  - cancel
  - pause
  - resume
  - compact
  - rewind target list
  - rewind to target
- browser frontend Console で、TUI と同等の操作が使える。
  - input 送信
  - completion lookup
  - cancel / pause / resume / compact
  - rewind target の取得
  - rewind target 選択と rewind 実行
- frontend/TUI の操作系は protocol transport を使い、個別 HTTP operation endpoint を主経路として使わない。
- 既存 HTTP operation endpoint を残す場合は compatibility/deprecation として明記され、frontend/TUI の通常操作からは外れている。
- rewind target list と rewind 実行結果が protocol event stream / diagnostics と整合する。
- unsupported diagnostic のまま残る操作がある場合は、仕様上 follow-up として明示されている。ただし上記必須操作については原則 unsupported のままにしない。
- runtime/backend/frontend/TUI の各変更に対して妥当な cargo check/test または frontend check を実行し、結果を thread に残す。

## 注意

- TUI 側の既存 keybinding / command / UX と揃える。
- browser frontend Console は、TUI と同じ操作概念を UI として持つ。
- Backend/runtime が受け付ける操作と Worker protocol method の対応を明確にする。
- HTTP endpoint を操作ごとに増やすだけの設計にしない。
- 既存の Backend Workspace authority 原則を崩さない。Worker/runtime 側に local `.yoi` discovery や client config discovery を再導入しない。
