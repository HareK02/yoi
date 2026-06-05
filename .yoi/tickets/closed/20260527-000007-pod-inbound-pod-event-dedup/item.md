---
id: 20260527-000007-pod-inbound-pod-event-dedup
slug: pod-inbound-pod-event-dedup
title: Inbound PodEvent ハンドリングの重複を統合する
status: closed
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:07Z
updated_at: 2026-05-30T05:37:00Z
assignee: null
legacy_ticket: tickets/pod-inbound-pod-event-dedup.md
---

## Migration reference

- legacy_ticket: tickets/pod-inbound-pod-event-dedup.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# Inbound PodEvent ハンドリングの重複を統合する

## 背景

子 Pod から `Method::PodEvent(event)` を受けたときの処理が `controller_loop` と `drive_turn` の 2 箇所にコピーされている。

`controller.rs:693-720`（idle / paused 中）:

```rust
Method::PodEvent(event) => {
    crate::ipc::event::apply_event_side_effects(
        &event, &spawned_registry, &spawner_name, &self_parent_socket,
    ).await;
    pod.push_pod_event_notify(event);
    if shared_state.get_status() == PodStatus::Idle {
        pending = Some(PendingRun::RunForNotification);
    }
}
```

`controller.rs:861-879`（in-flight turn 中）:

```rust
Some(Method::PodEvent(event)) => {
    let self_parent_socket = parent_socket.cloned();
    crate::ipc::event::apply_event_side_effects(
        &event, spawned_registry, self_name, &self_parent_socket,
    ).await;
    notify_buffer.push_pod_event(event);
}
```

差分は 2 点:

1. **buffer への push 経路**: `pod.push_pod_event_notify(event)` vs `notify_buffer.push_pod_event(event)`。両者は同じ `NotifyBuffer` を叩く（`pod.rs:845-846` は `self.pending_notifies.push_pod_event(event)` を呼ぶだけで、`notify_buffer_handle()` はその `pending_notifies.clone()` を返す）。**完全に等価**。
2. **auto-kick**: idle 経路だけ `PendingRun::RunForNotification` を stage する。in-flight 経路は in-flight 自体が消化するので不要。

つまり「event の処理本体」（side-effects + notify buffer への push）は同一で、後段の auto-kick だけが state-dependent な分岐。にもかかわらず関数化されておらず、片方をいじってもう片方を忘れると挙動が割れる。

## 要件

- side-effects 適用 + NotifyBuffer への typed push の流れを単一関数 `handle_inbound_pod_event` に切り出す。
- `controller_loop` / `drive_turn` の両方からこのヘルパーを呼ぶ形に置き換える。
- auto-kick (`PendingRun::RunForNotification` の stage) は呼び出し側の責務として残す。これは Pod のライフサイクル状態に依存した判断で、ヘルパー内には押し込めない。
- 関数シグネチャは引数を最小化する。`event`、`spawned_registry`、`self_name: &str`、`self_parent_socket: &Option<PathBuf>` または `Option<&PathBuf>`、`notify_buffer: &NotifyBuffer` の 5 つで足りる前提。`Pod` への可変参照は不要（`notify_buffer` で代用可能）。
- 動作変化なし。既存の `Method::PodEvent` 挙動（in-flight / idle 両方）が完全に同一で続行すること。

## 完了条件

- `controller.rs` 内に `apply_event_side_effects` 呼び出しが 1 箇所だけ残り、`controller_loop` と `drive_turn` の `Method::PodEvent` アームはどちらも `handle_inbound_pod_event(...)` 呼び出し + idle 経路のみ auto-kick stage、という形になる。
- 既存の inbound PodEvent 関連テスト（特に `apply_event_side_effects` の idempotency や `notify_buffer` への typed push）が通る。

## 範囲外

- `apply_event_side_effects` 自体の中身変更。
- `NotifyBuffer` API のリネーム / 統合。
- `pod.push_pod_event_notify` の削除（[[pod-interrupt-prep-internalize]] と同じく将来の整理対象だが、本チケットでは外向き API は触らない）。
