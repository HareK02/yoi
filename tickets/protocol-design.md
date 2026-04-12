# Protocol の設計

## 背景

現状の Protocol (`Method` / `Event`) は最低限のストリーミングイベントのみ。
機能が増えるにつれ、以下が不足している:

- Compact 発生時のクライアント通知
- Permission の ask/reply フロー
- セッション切り替え（compact 後の新 session_id 通知）
- クライアント→Pod の制御拡張（設定変更等）

## 現状の Protocol

### Method (Client → Pod)

```rust
Method::Run { input }
Method::Resume
Method::Cancel
Method::GetHistory  // request-response（socket 層で直接応答）
```

### Event (Pod → Client)

```
TurnStart, TurnEnd, TextDelta, TextDone,
ToolCallStart, ToolCallArgsDelta, ToolCallDone, ToolResult,
Usage, RunEnd, Error, History
```

## 設計課題

### 1. Broadcast vs Request-Response の区別

現状:
- Event は全て broadcast channel 経由
- `GetHistory` だけ socket 層で直接応答（暗黙の request-response）

課題:
- request-response が増えると socket_server の分岐が膨らむ
- クライアント側で「この Event は自分のリクエストへの応答」と判別できない

選択肢:
- **A. 現状維持**: request-response は socket 層で個別に処理。シンプルだがスケールしない
- **B. request_id パターン**: Method に optional `id` を持たせ、応答 Event に同じ `id` を含める
- **C. 型で分ける**: `Response` enum を Event とは別に定義

### 2. Compact イベント

TUI が compact の進行を表示するために必要:

```rust
Event::CompactStart  // compact 開始
Event::CompactDone { new_session_id: String }  // 成功
Event::CompactFailed { error: String }  // 失敗
```

compact は Pod 内部で自律的に発火するので、broadcast で全クライアントに通知が自然。
CompactDone 後、クライアントは GetHistory で新しい history を取得できる。

### 3. セッション情報の通知

compact で session_id が変わる。クライアントに通知する方法:

- **CompactDone に含める**: `Event::CompactDone { new_session_id }`
- **汎用 SessionChanged イベント**: compact 以外でも session_id が変わるケース（fork 等）に対応

### 4. Permission の ask/reply（将来）

permission-extension-point チケットで扱う。ここでは Protocol の拡張パターンだけ意識:

```
Pod → Client: Event::PermissionRequest { id, tool, args }
Client → Pod: Method::PermissionReply { id, allow }
```

これは request-response の逆方向（Pod が要求元）。同じソケット上で双方向に使える。

## 検討事項

- Event の肥大化をどう管理するか（カテゴリ分け？ネスト？）
- Protocol のバージョニング（クライアント互換性）
- イベントの順序保証（broadcast channel の特性）
