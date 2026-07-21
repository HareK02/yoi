---
title: 'TUI backend runtime worker listを表面化する'
state: 'closed'
created_at: '2026-07-18T02:39:04Z'
updated_at: '2026-07-21T09:16:15Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T02:39:40Z'
---

## 背景

TUI を backend runtime の client として接続する経路は基礎実装があるが、ユーザーが通常導線として worker を一覧し、既存 worker を選んで接続/確認する入口が表面化していない。

直近の調査では、backend/runtime の worker summary/detail は REST で取れる一方、会話内容は observation WebSocket snapshot を読む必要があり、CLI/TUI から人間向けに扱いづらいことも確認された。

この Ticket では、まず TUI backend runtime client 導線の worker list 実装から着手し、現行 staging/extract 実装が runtime/embedded 経路で破綻していないかの確認も含める。

## 実装要件

- TUI を backend runtime client として使う導線で、runtime worker list をユーザーに見える形にする。
- worker list は backend が公開している runtime/worker summary を authority とし、TUI 側で独自 scheduler や duplicate backend を作らない。
- list item には少なくとも runtime id、worker id、label/profile、status/state、working directory summary を表示できること。
- 既存 worker を選択して attach/read/inspect へ進めるための実装境界を整理する。今回の主作業は list で、attach/read の大規模実装は必要なら follow-up に分ける。
- backend/runtime は local/private boundary を維持し、frontend だけ外部 bind できる前提を壊さない。
- staging/extract 実装について、embedded/runtime 経路で staging write abstraction が破綻していないかを確認し、必要なら小修正する。

## 非目標

- worker scheduler を TUI に新設しない。
- backend/runtime REST API を無秩序に増やさない。必要な場合は bounded/read-only な endpoint として理由を明確にする。
- TUI の全面 redesign はしない。
- extract candidate tuning はこの Ticket の主目的ではない。

## 受け入れ条件

- backend runtime client mode で worker list が見える、またはそのための CLI/TUI 実装差分が明確に入る。
- list は backend/runtime authority 由来の worker identity を使う。
- 実装者が既存 worker `arc/3` 相当を確認し、動作確認結果を Ticket thread に残す。
- staging/extract の current implementation について、runtime/embedded 経路での確認結果を Ticket thread に残す。
- `cargo fmt --check` と relevant tests が通る。
- code/resource 変更がある場合は `nix build .#yoi` を通す。
