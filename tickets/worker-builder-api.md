# Worker API: Builder パターンによるキャッシュ保護の自動化

## 背景

現状の `Worker` は Type-state パターンで `Mutable` / `CacheLocked` の2状態を持つが、
`lock()` を呼ばなくても `run()` できてしまうため、知らないユーザーは最適でないパスを通る。
"Pit of success" になっていない。

## 方針

Builder パターンで「設定フェーズ」と「実行フェーズ」を自然に分離する。

```rust
// 設定フェーズ（自由に編集可能）
let worker = Worker::builder(client)
    .system_prompt("You are a helpful assistant.")
    .tool(my_tool)?
    .hook(my_hook)
    .build();  // ← ここで自動的にcache-protected状態へ

// 実行フェーズ（prefix不変、キャッシュ保護済み）
worker.run("Hello").await?;

// 再設定が必要な場合
let builder = worker.reconfigure();
builder.tool(another_tool)?;
let worker = builder.build();
```

## 設計ポイント

- `build()` 後は常にキャッシュ保護状態。lock の存在をユーザーが意識する必要がない
- 設定と実行の分離が型で強制される（コンパイル時保証）
- `reconfigure()` でビルダーに戻せるため、動的なツール追加等にも対応可能
- Type-state の恩恵は維持しつつ、APIの認知負荷を下げる

## ToolDefinition ファクトリの遅延初期化

現状 `register_tool()` 時に `ToolDefinition` のファクトリクロージャが即時呼び出しされている。
Builder パターンへの移行に伴い、`build()` 時（= セッション開始時）まで遅延させる。

- `builder.tool(definition)` は定義を蓄積するだけ
- `build()` でファクトリを一括実行し、ToolServer を構築

## 移行

- 現行の `lock()` / `unlock()` は deprecated → 次メジャーで削除
- `Worker::new()` は `Worker::builder()` へ移行
