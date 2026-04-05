# ツール出力の遅延読み込み設計

## 課題

ツール実行結果（ファイル内容、検索結果等）は サイズが予測不能 で、
全量を `Item::ToolResult { output: String }` として LLM コンテキストに
載せると、トークン消費が爆発する。

## 方針

- ツール出力に **Inline / Stored** の区別を導入する
- Stored な出力は **BlobStore** に保存し、履歴には要約のみ載せる
- LLM が詳細を見たい場合は **inspect ツール** で部分取得する

## データ型

### ToolOutput（llm-worker 側）

```rust
pub enum ToolOutput {
    /// 小さな結果: そのまま history に載る
    Inline(String),
    /// 大きな結果: summary だけ history に載り、全体は BlobStore に保存される
    Stored {
        summary: String,
        content: Content,
    },
}

pub enum Content {
    Text(String),
    Structured(serde_json::Value),
}
```

- `Tool::execute()` の戻り値は `Result<String, ToolError>` のまま据え置き
- `From<String> for ToolOutput` で閾値ベースの自動昇格を行う
- ツール実装者が明示的に `ToolOutput` を返したい場合は別トレイトメソッドを用意

### BlobStore（llm-worker-persistence 側）

```rust
pub type BlobId = uuid::Uuid;  // UUID v7

pub trait BlobStore: Send + Sync {
    fn store(&self, content: &Content) -> impl Future<Output = Result<BlobId, BlobStoreError>> + Send;
    fn load(&self, id: BlobId) -> impl Future<Output = Result<Content, BlobStoreError>> + Send;
    fn exists(&self, id: BlobId) -> impl Future<Output = Result<bool, BlobStoreError>> + Send;
}
```

### FsBlobStore レイアウト

```
blobs/
├── {blob_id}.txt    # Content::Text
└── {blob_id}.json   # Content::Structured
```

セッションとは独立したフラットなストア。セッションとの紐付けは
ログ側の参照（summary 内の `[blob:<id>]`）で行う。

## 自動サマリ

`From<String>` による自動昇格時のサマリ生成ルール:

| 項目 | 値 |
|---|---|
| Inline 閾値 | 800 bytes |
| サマリ上限 | 400 bytes |
| 先頭行数 | 5 行 |
| 末尾行数 | 3 行 |

### Text のサマリ形式

```
[blob:<id>] text | {N} lines
── head ──
{先頭5行}
── tail ──
{末尾3行}
```

### Structured (JSON Array) のサマリ形式

```
[blob:<id>] json_array | {N} entries
── schema ──
{最初の要素のキー: 型}
── head ──
{先頭2要素}
```

### Structured (JSON Object) のサマリ形式

```
[blob:<id>] json_object | {N} keys
── keys ──
{キー一覧と各値の型/サイズ}
```

## Worker への統合

```
Tool::execute() → Result<String, ToolError>
       │
       ▼  From<String> for ToolOutput
  ToolOutput::Inline(s)     ← len ≤ 800
  ToolOutput::Stored { .. } ← len > 800
       │
       ▼  Worker が BlobStore に保存
  Item::ToolResult { output: summary }  ← history に載る
       │
       ▼  LLM が詳細を見たい場合
  inspect(blob_id, selector?) → 部分取得
```

Worker はオプショナルに `BlobStore` を保持する。
BlobStore が未設定の場合は従来通り全量 Inline として扱う。

## inspect ツール

Worker に BlobStore が設定されている場合、自動的に登録される組み込みツール。

```
inspect(blob_id, selector?)
```

- selector 省略: メタ情報 + 先頭部分
- `lines:20-50`: 行範囲（Text 用）
- `slice:3..8`: インデックス範囲（Array 用）
- `key:results`: キー指定（Object 用）
