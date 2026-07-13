---
title: 'Use live Worker snapshot for runtime observation'
state: 'inprogress'
created_at: '2026-07-13T17:48:09Z'
updated_at: '2026-07-13T17:48:47Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T17:48:47Z'
---

## 背景

Browser Console は worker-runtime REST/WebSocket observation endpoint から connect-time snapshot を受け取る。現状の `Runtime::worker_observation_snapshot` は live WorkerController の session-log sink を見ず、stub snapshot (`entries: []`) を返している。そのため Worker restore 後に文脈は engine/session log 上で保持されていても、console snapshot entries は空になる。

TUI/socket attach と同じく、Runtime observation snapshot も live WorkerHandle の `SegmentLogSink` / greeting / status / in-flight snapshot から構築する必要がある。

## 要件

- `worker-runtime` observation snapshot は live execution backend の Worker snapshot を使う。
- Real worker backend は `WorkerHandle` から `Event::Snapshot` を構築する。
- Snapshot entries は `SegmentLogSink::subscribe_with_snapshot` の prefix を JSON 化したものにする。
- Live Worker がない場合だけ existing fallback/stub を使う。
- Browser Console reconnect/restore 時に snapshot entries が空にならない。

## 受け入れ条件

- `Runtime::worker_observation_snapshot` が backend-provided snapshot を優先する。
- worker-runtime tests が通る。
- workspace frontend console tests が通る。
