---
title: 'Backend runtime経由でWorker protocol操作をTUI同等に使えるようにする'
state: 'planning'
created_at: '2026-07-21T09:20:07Z'
updated_at: '2026-07-21T09:42:18Z'
assignee: null
readiness: 'draft'
---
## 背景

Backend runtime worker list / attach 経路が入ったので、次は attach 後の Worker control 操作を TUI 同等に Backend/runtime 経由でも使えるようにする。

現在の local TUI attach は Worker protocol の `Method` を送る操作を持っている。一方、Backend runtime attach client は一部操作を ad-hoc な HTTP endpoint に map し、pause / resume / rewind 系は unsupported diagnostic にしている。ここを、Backend 経由でも TUI と同じ protocol 操作を送れる transport として整える。

## 要件

- Backend/runtime 経由の Worker protocol transport を定義・実装する。
  - 既存 event WebSocket が read-only observation なら、双方向 WS を追加するか、明確な command WS を追加する。
  - transport payload は既存 `protocol::Method` / `protocol::Event` との対応を保つ。
  - REST endpoint の増設が必要な場合も、主設計は「個別 HTTP API」ではなく「protocol method を Backend/runtime に中継する経路」として扱う。
- Backend/runtime が以下の Worker protocol 操作を受け付ける。
  - cancel
  - pause
  - resume
  - rewind target list
  - rewind to target
- TUI Backend Runtime client の unsupported 扱いを解消し、既存 TUI 操作から Backend 経由でも同等に送れるようにする。
- browser/frontend でも TUI と同等の操作ができるようにする。
  - attach 中 Worker に対する cancel / pause / resume
  - rewind target の取得
  - rewind target 選択と rewind 実行
- authority は Backend/runtime 側の Worker identity / event stream / protocol transport とする。
- frontend/TUI が独自 scheduler や duplicate runtime authority を作らない。

## 受け入れ条件

- Backend runtime attach 中の TUI で、local Worker attach と同等の cancel / pause / resume / rewind 操作が使える。
- browser/frontend で、attach 中 Worker に対して cancel / pause / resume / rewind 操作が使える。
- rewind target list と rewind 実行結果が protocol event stream / diagnostics と整合する。
- unsupported diagnostic のまま残る操作がある場合は、仕様上 follow-up として明示されている。
- runtime/backend/frontend/TUI の各変更に対して妥当な cargo check/test または frontend check を実行し、結果を thread に残す。

## 注意

- TUI 側の既存 keybinding / command / UX と揃える。
- Backend/runtime が受け付ける操作と Worker protocol method の対応を明確にする。
- HTTP endpoint を操作ごとに増やすだけの設計にしない。必要なら compatibility layer として扱う。
- 既存の Backend Workspace authority 原則を崩さない。Worker/runtime 側に local `.yoi` discovery や client config discovery を再導入しない。
