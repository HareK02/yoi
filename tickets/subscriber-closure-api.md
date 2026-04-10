# Subscriber API: クロージャベースのスコープ表現

## 背景

Block系イベントは時間的に排他（TextBlock中にToolUseBlockは来ない）で、
Meta系イベント（Usage等）はいつでも流れ得る。
現行の `Handler<K>` + `Scope: Default` はこの保証を実現しているが、
ユーザーから見ると Kind/Scope/型消去のボイラープレートが重く、
`Timeline` と `WorkerSubscriber` の2層が「どちらを使えばいいか」分かりにくい。

## 方針

クロージャでスコープの寿命を表現し、ブロックの時間的排他性を Rust の借用で自然に保証する。

```rust
// Block系: クロージャ引数 = スコープの寿命保証
worker.on_text_block(|block| {
    block.on_delta(|text| print!("{}", text));
    block.on_stop(|reason| println!("\n---"));
});

worker.on_tool_use_block(|block| {
    block.on_delta(|json| { /* ... */ });
    block.on_stop(|call| { /* ... */ });
});

// Meta系: スコープ不要（いつでも来る）
worker.on_usage(|usage| { /* ... */ });
```

## 設計ポイント

- ブロックのスコープ = クロージャの借用寿命。ユーザーは `Kind` / `Scope: Default` を知らなくていい
- Block系の排他性が「ブロックが始まったらクロージャが呼ばれ、終わったら抜ける」という直感に一致する
- Meta系はフラットなコールバック。スコープ管理不要
- 内部的には現行の `Handler<K>` + `Timeline` ディスパッチ機構を維持し、クロージャを Handler に変換するアダプタを挟む
- `WorkerSubscriber` トレイト + 5種の SubscriberAdapter ボイラープレートが不要になる

## 現行からの変更

- `Timeline` は `pub mod` → 内部実装に格下げ（Worker の実装詳細に閉じ込める）
- `WorkerSubscriber` トレイト → 廃止。クロージャ登録 API に置き換え
- `Handler<K>` トレイトは内部で維持（クロージャからの変換先として）
