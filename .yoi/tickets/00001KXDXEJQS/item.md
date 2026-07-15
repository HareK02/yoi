---
title: 'Render console snapshot entries'
state: 'closed'
created_at: '2026-07-13T14:17:43Z'
updated_at: '2026-07-15T16:19:27Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T14:18:30Z'
---

## 背景

Browser Console は Worker observation WebSocket から connect-time `snapshot` event を受け取っているが、`snapshot.data.entries` を conversation surface に投影していない。そのため reconnect / restore 後、snapshot 自体は届いていても console が空に見える。現在は snapshot の status と in-flight blocks だけを使っている。

Protocol の `Event::Snapshot.entries` は session-store `LogEntry` JSON であり、bulk reconstruction lane として UI が派生 view を seed するためのデータである。Browser Console でも TUI と同様に snapshot entries から committed conversation rows を復元する。

## 要件

- Browser Console projection が `snapshot.data.entries` を render する。
- `SegmentStart.history` も replay し、fork/restore/compaction 後の seed history を表示する。
- UserInput / assistant message / reasoning / tool call / tool result を既存 ConsoleLine に変換する。
- Snapshot の in-flight block 表示は維持する。
- 不明な snapshot entry は error にせず無視する。

## 受け入れ条件

- snapshot entries だけで user / assistant / tool rows が console に表示される。
- snapshot in-flight blocks は引き続き streaming line として表示される。
- frontend tests が通る。
