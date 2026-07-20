---
title: 'Enable Ticket tools in default profile'
state: 'closed'
created_at: '2026-07-18T03:04:36Z'
updated_at: '2026-07-18T08:32:27Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T03:05:10Z'
---

## 背景

通常の `builtin:default` Worker では Ticket feature が無効で、`TicketList` / `TicketShow` などの typed Ticket tools が露出しない。実装作業では Ticket の前提・受け入れ条件・thread を読めないと正しく作業できないため、default profile でも Ticket tools を有効化する。

Orchestrator 固有の relation / orchestration plan tools は引き続き `ticket_orchestration` で分離し、default profile では有効化しない。

## 要件

- `resources/profiles/default.dcdl` の Ticket feature を有効化する。
- builtin default profile artifact の Ticket feature も同じ既定にする。
- setup wizard が生成する `user:default` profile でも Ticket feature を有効化する。
- `ticket_orchestration` は default では無効のままにする。
- 裸の `WorkerManifestConfig::builtin_defaults()` の feature default は profile 既定とは別なので変更しない。

## 受け入れ条件

- `builtin:default` profile resolution で `feature.ticket.enabled == true` になる。
- `builtin:default` profile resolution で `feature.ticket.access == lifecycle` になる。
- `builtin:default` profile resolution で `feature.ticket_orchestration.enabled == false` のままになる。
- setup model が生成する default profile に `[feature.ticket] enabled = true` が含まれる。
- 関連する profile/setup focused tests が通る。
