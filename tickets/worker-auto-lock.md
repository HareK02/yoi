# Worker: run() 時の自動キャッシュロックと ToolDefinition ファクトリ遅延初期化

## 背景

現状の `Worker` は Type-state パターンで `Mutable` / `CacheLocked` の2状態を持つが、
`lock()` を呼ばなくても `run()` できてしまうため、キャッシュ保護の存在を知らないユーザーは
常に非最適パスを通ることになる。

また `register_tool()` 時に `ToolDefinition` のファクトリクロージャが即時呼び出しされており、
本来の意図である遅延初期化になっていない。

## 方針

### run() 時の自動ロック

`run()` の冒頭で、`Mutable` 状態なら自動的に `CacheLocked` へ遷移する。
これにより lock を知らないユーザーでも嫌でもキャッシュ保護される。

ターンの合間に history や system prompt を編集したい場合は、明示的に `unlock()` を挟む。
次の `run()` で再び自動ロックされる。

```rust
let mut worker = Worker::new(client);
worker.set_system_prompt("...");
worker.register_tool(my_tool)?;

// Mutable のまま run() → 自動で lock される
worker.run("Hello").await?;

// ターン間で内容を弄りたい場合
worker.unlock();
worker.history_mut().truncate(5);
// 次の run() で再 lock
worker.run("Continue").await?;
```

#### 設計ポイント

- `run()` が `&mut self` を取る以上、内部で状態遷移しても外部の型は変わらない。
  実装は内部フラグ（`is_locked: bool`）で管理し、`Mutable` / `CacheLocked` の
  type-state はそのまま維持する。`Worker<C, Mutable>` の `run()` が内部で lock 相当の
  処理を行い、`unlock()` が呼ばれるまでキャッシュ破壊的な操作を（ランタイムで）ブロックする
- `Worker<C, CacheLocked>` は従来どおり。明示的に `lock()` してから使うパスも残る
- interceptor, max_turns, callbacks 等キャッシュに影響しない設定は lock 状態でも自由に変更可能

### ToolDefinition ファクトリの遅延初期化

`register_tool()` は定義を蓄積するだけにし、ファクトリの実行を初回 `run()` まで遅延させる。

- `register_tool()` → `ToolDefinition` を Vec に push するだけ
- 初回 `run()` の自動 lock 時にファクトリを一括実行し、ToolServer を構築
- `unlock()` 後に追加登録された tool は次の `run()` で初期化

## 移行

- `lock()` / `unlock()` は引き続き使える（明示的なキャッシュ管理用）
- `Worker::new()` → `run()` のパスが自動保護されるため、既存コードは変更不要
