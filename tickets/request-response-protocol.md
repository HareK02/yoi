# Protocol: request-response パターン導入

## 背景

現在の Pod Protocol は fire-and-forget（Method 送信）+ broadcast（Event 受信）のみ。
クライアントが Pod に問い合わせて応答を受け取る request-response パターンがない。

これが必要になるケース:
1. **GetHistory**: TUI 接続時にセッション履歴を取得
2. **Permission ask**: ツール実行の許可をクライアントに問い合わせ（permission-extension-point チケット）

## 設計

### 方式: handle_connection 内での直接応答

broadcast channel を変更せず、接続ごとの writer に直接返す。

```rust
// handle_connection 内
match method {
    Method::GetHistory => {
        // broadcast を経由せず、要求元の writer に直接返す
        let items = handle.shared_state.history();
        writer.write(&Event::History { items }).await;
    }
    other => handle.send(other).await,  // 既存: controller へ転送
}
```

- broadcast の仕組みに手を入れない
- 読み取り系は SharedState から直接返せる
- controller を経由する必要がない

### Protocol 変更

```rust
// Method 追加
enum Method {
    Run { input: String },
    Resume,
    Cancel,
    GetHistory,              // NEW
    // 将来: PermissionReply  // permission チケットで追加
}

// Event 追加
enum Event {
    // ... 既存 ...
    History { items: Vec<Item> },  // NEW: GetHistory への応答
}
```

### TUI 接続フロー

```
TUI connect
  → send GetHistory
  ← recv History { items }    ← 直接応答（この接続のみ）
  → 履歴表示
  ← recv TextDelta, ...       ← broadcast（通常のイベントストリーム）
```

## 将来の拡張

Permission の `ask` は双方向のやりとりが必要で、より複雑:
- Pod → Client: `Event::PermissionRequest { id, tool, args }`
- Client → Pod: `Method::PermissionReply { id, allow: bool }`

これは request-response の逆方向（Pod が要求元）になるが、同じソケット上の双方向通信として自然に実現できる。詳細は permission-extension-point チケットで扱う。
