# protocol: JSONL ストリーム変換ユーティリティ

## 背景

protocol クレートは現在型定義のみ。JSONL の読み書き処理（BufReader + lines + デシリアライズ / シリアライズ + `\n` + write_all）が socket_server.rs と client.rs で重複している。空行スキップやエラー変換のロジックも各所にベタ書き。

## 方針

`protocol::stream` モジュールに `JsonLineReader` / `JsonLineWriter` を追加し、JSONL 変換を protocol クレートの責務に含める。

```rust
// protocol::stream

pub struct JsonLineReader<R> { /* BufReader<R> */ }
pub struct JsonLineWriter<W> { /* W */ }

impl<R: AsyncBufRead + Unpin> JsonLineReader<R> {
    pub fn new(reader: R) -> Self;
    pub async fn next<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Error>;
}

impl<W: AsyncWrite + Unpin> JsonLineWriter<W> {
    pub fn new(writer: W) -> Self;
    pub async fn write<T: Serialize>(&mut self, value: &T) -> Result<(), Error>;
}
```

## 設計ポイント

- feature gate にはしない。このプロジェクトで protocol を tokio なしで使う場面がない
- tokio の依存は `io-util` のみ（`AsyncBufRead`, `AsyncWrite` に必要な最小限）
- 空行スキップ、改行付与、serde エラーの IO エラー変換を一箇所に集約
- `JsonLineReader` は内部で `BufReader` を持つ。呼び出し側で `BufReader` を作る必要がない

## 変更対象

- `crates/protocol/Cargo.toml` — tokio (io-util) 依存を追加
- `crates/protocol/src/stream.rs` — 新規モジュール
- `crates/protocol/src/lib.rs` — `pub mod stream`
- `crates/pod/src/socket_server.rs` — `JsonLineReader` / `JsonLineWriter` に置き換え
- `crates/tui/src/client.rs` — 同上
- `crates/pod/tests/controller_test.rs` — テストのストリーム処理も置き換え
