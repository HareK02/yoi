# Protocol の設計

## 背景

現状の Protocol (`Method` / `Event`) は最低限のストリーミングイベントのみ。
機能が増えるにつれ、以下が不足している:

- Compact 発生時のクライアント通知
- Permission の ask/reply フロー（permission-extension-point の段階 3）
- セッション切り替え（compact 後の新 `session_id` 通知）
- クライアント→Pod の制御拡張（設定変更等）

## 現状（調査結果）

### Method (Client → Pod)

`crates/protocol/src/lib.rs:11-32`

```
Run { input } | Notify { message } | PodEvent(PodEvent)
Resume | Cancel | Pause | Shutdown | GetHistory
```

### Event (Pod → Client)

`crates/protocol/src/lib.rs:85-134`

```
TurnStart/TurnEnd, TextDelta/TextDone,
ToolCallStart/ArgsDelta/Done, ToolResult,
Usage, RunEnd, Error, History, Notification, Shutdown
```

### 既存の request-response の扱い

`crates/pod/src/socket_server.rs:96-113` の `GetHistory` だけが
「socket 層で即応答」。他の Method は `handle.send(method)` で
Controller に fire-and-forget。

Client (`crates/tui/src/client.rs`) は reader を単一の mpsc に流すだけで、
応答と broadcast を区別しない。`GetHistory` は 1 回きりで競合しないため
現状は動く。

### Compact の現状

`crates/pod/src/pod.rs:825` の `compact()` は新しい `SessionId` を戻すが、
ネットワークに一切乗らない:

- mid-turn: `do_compact_and_resume` (685行) — 失敗時のみ `Notification::Error`
- post-turn: `try_post_run_compact` (715行) — 失敗時のみ `Notification::Warn`

成功時は完全にサイレント。`Event::History` / `Greeting` に `session_id` は入っていないため、
TUI は現在の session_id を認識できない。

## 決定事項

### 1. request-response の扱い — 今は足さない

- 現状 `GetHistory` が 1 回きりで競合しないため、新しい wire フィールドなしで動く。
- 将来 Permission ask/reply などが入る時点で、**Method/Event 双方に `request_id: Option<String>` を追加**する方針を採る。既存実装は無視するだけで互換を保てる。
- `Response` を別 enum にする案 (C) は採用しない:
  - 先例として `PodEvent` が `Method::PodEvent(PodEvent)` で Method 側に包まれており、第 3 の wire 型を増やす理由がない。
  - Reader の分岐が増え、`PodClient` の mpsc 設計を崩す。

今回は **ドキュメントとしての宣言のみ**。フィールドは足さない。

### 2. Compact イベント — 追加する

```rust
Event::CompactStart
Event::CompactDone { new_session_id: SessionId }
Event::CompactFailed { error: String }
```

- 発火点は `do_compact_and_resume` と `try_post_run_compact` の 2 箇所。
  `event_tx.send(...)` を start / 成功 / 失敗に挟むだけ。
- 既存の `Notification::Error/Warn`（compactor source）はイベント側に移行してよいが、
  初期移行では重複して出しても害はない。先に Event を足し、Notification は後続チケットで整理。
- Broadcast で全クライアントに通知（compact は Pod 自律発火で、特定リクエストへの応答ではない）。

### 3. session_id 通知 — CompactDone に含める

- `Event::CompactDone { new_session_id }` に載せる。
- 汎用 `SessionChanged` は作らない（YAGNI — fork 等が実装される時まで判断を保留）。
- TUI 側は現状 session_id を利用していないので、イベントを受け取るだけでよい（将来 GetHistory 再取得などに使う）。

### 5. wire 型 — `protocol` が `uuid::Uuid` を直接扱う

- `SessionId` のパースは `protocol` クレートの責務。`protocol/Cargo.toml` に
  `uuid = { workspace = true, features = ["serde"] }` を追加し、
  `Event::CompactDone { new_session_id: uuid::Uuid }` として型付けする。
- `session-store::SessionId` は `uuid::Uuid` のエイリアスなので、Pod 側は変換なしで渡せる。
- 文字列経由にはしない（wire を弱く型付けして嬉しいことがない）。

### 6. broadcast 手段 — Pod が `event_tx` を直接保持

- `Notifier` は `Event::Notification` の replay バッファ専用のまま残し、compact 系イベントは
  通さない（意味が噛み合わない）。
- `Pod` に `event_tx: Option<broadcast::Sender<Event>>` を持たせ、
  `attach_notifier` と同じタイミングで Controller 側から渡す。
- compact 発火点では `self.event_tx.as_ref().map(|tx| tx.send(...))` で直接流す。

### 7. late subscriber への再配信 — しない

- compact イベントはバッファせず、broadcast 時点で購読していないクライアントには届かない。
- TUI は現状 session_id を使っていないため、接続後に直近の compact を知る必要がない。
- 必要になった時点（fork や複数クライアント同時接続が現実になった時）で別チケットで buffer 化。

### 4. Permission ask/reply — 別チケットで実装

- permission-extension-point の段階 3 で追加する。
- Pod → Client は `Event::PermissionRequest { id, tool, args }`、
  Client → Pod は `Method::PermissionReply { id, allow }` を想定。
- ここで使う `id` は 1. の `request_id: Option<String>` パターンに従う。
  protocol-design 本チケットでは足さず、permission-extension-point 側で導入する。

## 本チケットで実装するもの

1. `protocol` クレートに `uuid = { workspace = true, features = ["serde"] }` を追加し、
   `Event::CompactStart` / `Event::CompactDone { new_session_id: uuid::Uuid }` /
   `Event::CompactFailed { error: String }` を追加する。
2. `Pod` に `event_tx: Option<broadcast::Sender<Event>>` を持たせ、Controller 側から
   `Notifier` と同じタイミングで渡す。
3. `crates/pod/src/pod.rs` の compact 発火 2 箇所（`do_compact_and_resume` /
   `try_post_run_compact`）で start / 成功（CompactDone） / 失敗（CompactFailed）を broadcast する。
4. `crates/tui/src/app.rs` の `handle_pod_event` に 3 分岐を追加し、
   既存の `NoticeWarn` / `NoticeError` と同じ枠で最低限のテキストを表示する。
   - `[compact] starting`
   - `[compact] done (new session <uuid先頭8字>)`
   - `[compact error] <message>`
5. テスト:
   - Event の JSON roundtrip（`protocol` クレート）: 3 バリアント + uuid の shape
   - 成功/失敗/mid-turn の各パスで Event が発行されることを確認する統合テスト（`pod` クレート）

## 本チケットで実装しないもの

- `request_id: Option<String>` フィールドの追加（将来、必要になったチケットで足す）
- `Event::SessionChanged`（fork 実装時に再検討）
- Notification と CompactFailed の重複整理（後続チケット）
- Permission ask/reply の wire 定義（permission-extension-point 段階 3）

## 検討メモ（将来向け）

- Event の肥大化が気になってきたら、`Event::Stream(StreamEvent)` のようにカテゴリ分けしてネストする余地はある。ただし現状 15 バリアントで破綻していないので先送り。
- Protocol のバージョニングは未着手。クライアントの互換性問題が顕在化した時点で `Event::Hello { protocol_version }` のような握手を追加する。
- Broadcast channel の特性上、遅い client では drop が起き得る。現状はログのみだが、compact 等の重要イベントを落とすとまずいので将来は per-client queue への置き換えを検討する。

## Review

- 状態: Approve with follow-up
- レビュー詳細: [./protocol-design.review.md](./protocol-design.review.md)
- 日付: 2026-04-21
