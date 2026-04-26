# Notification 用語のリネーム

## 背景

Pod 周辺に二系統の「notification」があり、用語が衝突して読みづらい:

- **outbound** (Pod → Client): `Event::Notification` / `Notifier::notify` — ユーザー向けの運用診断（compaction 失敗、tool 出力 truncated 等）
- **inbound** (External → Pod): `Method::Notify` / `NotificationBuffer` — 外部から Pod の LLM context に「メモを置く」経路

両者とも `Notification` / `Notify` を共有しており、コードを読む側が毎回どちらの経路かを文脈から判別する必要がある。

`Method::Notify` の動詞名は「呼び出し側がやる行為」として意味的に正しいので残し、**outbound 側の名詞**を `Alert` に倒して非対称を入れることで区別する。

## 要件

### outbound (Pod → Client) — `Alert` にリネーム

protocol:

- `Event::Notification(Notification)` → `Event::Alert(Alert)`
- `protocol::Notification` 構造体 → `protocol::Alert`
- `NotificationLevel` → `AlertLevel`
- `NotificationSource` → `AlertSource`

pod:

- `crates/pod/src/ipc/notifier.rs` → `alerter.rs`
- `Notifier` → `Alerter`
- `Notifier::notify(...)` → `Alerter::alert(...)`
- `Notifier::subscribe_with_snapshot()` の戻り値 `Vec<Notification>` → `Vec<Alert>`

tui:

- `Block::Notification` → `Block::Alert`（描画スタイルは変更しない）

### inbound (External → Pod) — `Notify` 据置 + 衛生整理

- `Method::Notify` 据置（動詞として正しい）
- `NotificationBuffer` → `NotifyBuffer`（メソッド名と揃える）
- `PendingNotification` → `PendingNotify`
- `notify_wrapper` プロンプト名 据置
- LLM 向け wrapper ラベル `[Notification]` 据置（LLM への見え方は変えない）

### protocol wire 互換

開発初期段階のため wire 互換は意図的に切る。client / pod を同時更新する前提で、旧名を吸収する `#[serde(rename)]` 等は入れない。

## 範囲外

- 通知の content / level / source の意味論変更（純粋な rename のみ）
- `Notifier` / `NotificationBuffer` の buffer 容量や snapshot 挙動の変更
- TUI 側 `Block::Alert` の描画スタイルの変更

## 完了条件

- 上記 rename がすべて反映されている
- ビルドが通り、既存テストが通る
- grep で旧名（outbound 文脈の `Notification` / `Notifier` / 動詞 `notify`）が消えている

## 参照

- `crates/protocol/src/lib.rs`（`Event::Notification`, `Notification` struct, `NotificationLevel/Source`）
- `crates/pod/src/ipc/notifier.rs`（outbound、`Notifier`）
- `crates/pod/src/ipc/notification_buffer.rs`（inbound、`NotificationBuffer`, `PendingNotification`）
- `crates/tui/`（`Block::Notification` 描画）

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./notification-naming-cleanup.review.md](./notification-naming-cleanup.review.md)
- 日付: 2026-04-26
