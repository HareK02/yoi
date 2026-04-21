---
name: Event broadcast pattern
description: Pod が protocol::Event を broadcast する公式パターン (Notifier と別経路)
type: project
---

Pod 内部から `protocol::Event` を broadcast する正規ルートは、`Pod` に
`event_tx: Option<broadcast::Sender<Event>>` を持たせて `attach_event_tx` で
Controller 側から注入する方式。`Notifier` は `Event::Notification` の
replay バッファ専用で、他イベントは通さない。

**Why:** `Notifier` は Notification 型の Warn/Error レベル情報 + late subscriber への
snapshot replay を責務にしており、Event 一般を乗せると意味が噛み合わない。
protocol-design チケットの決定事項 6/7 で確定 (2026-04-21)。

**How to apply:** 新しい Pod 発の Event を追加するときは、
1) `Pod::send_event(&self, event)` ヘルパ (`pod.rs:370-374`) を使う、
2) Controller は `pod.attach_notifier` の直後に `pod.attach_event_tx` を呼ぶ、
3) late subscriber への届きは期待しない (buffer 化が必要なら別チケット化)。
Notifier 経由で新種 Event を流す PR が来たら差し戻し対象。
