# ツール出力の設計

## 課題

ツール実行結果（ファイル内容、検索結果等）はサイズが予測不能で、
全量を LLM コンテキストに載せるとトークン消費が爆発する。

## 方針

ツール出力を **summary（常駐）** と **content（prunable）** の2フィールドに分離する。

- summary: 1-2行。常に history に残る。Prune 後もこれだけで「何をしたか」がわかる
- content: 詳細な出力。一定閾値まで。Prune で消える

巨大な出力（大量の grep 結果、巨大ファイル等）はフレームワークの責務外。
ツール側がファイルに書き出し、content に見取り図を置く。

## データ型

### ToolOutput

```rust
/// ツール実行結果。
///
/// summary は常に必須。content は省略可能。
/// Prune 時に content が除去され、summary だけが残る。
pub struct ToolOutput {
    /// 1-2行の要約。Prune 後も history に残る。
    /// 例: "read_file: src/main.rs — 42 lines"
    /// 例: "bash: cargo test — exit 0, 3 passed"
    /// 例: "grep: TODO in src/ — 128 hits, saved to /tmp/grep_result.txt"
    pub summary: String,

    /// 詳細な出力内容。Prune で消える。
    /// None の場合、summary のみが history に載る。
    pub content: Option<String>,
}
```

### Item::ToolResult

```rust
Item::ToolResult {
    id: Option<ItemId>,
    call_id: CallId,
    /// 1-2行の要約。Prune 後も残る。
    summary: String,
    /// 詳細な出力。Prune で None に置換される。
    content: Option<String>,
}
```

LLM への送信時は summary + content を結合して単一文字列にする。
content が None の場合は summary のみ。

```rust
impl Item {
    /// LLM に送信する出力文字列を構築。
    pub fn tool_result_text(&self) -> Option<&str> {
        match self {
            Item::ToolResult { summary, content: Some(c), .. } => {
                // 呼び出し側で結合
                None // 実際は format!("{summary}\n{c}")
            }
            Item::ToolResult { summary, content: None, .. } => Some(summary),
            _ => None,
        }
    }
}
```

### Tool trait の変更

`Tool::execute()` の戻り値を `Result<ToolOutput, ToolError>` に変更する。

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError>;
}
```

ツールが独自の summary を付けたい場合は `ToolOutput` を直接構築する。
単純なケースでは `From<String>` で自動変換できる: `Ok("result".to_string().into())`

### From\<String\> 変換

`From<String>` による自動変換:

```rust
impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        if s.len() <= SUMMARY_THRESHOLD {
            // 小さい出力: summary のみ（content なし）
            ToolOutput { summary: s, content: None }
        } else {
            // summary = 先頭行 + メタ情報
            let lines = s.lines().count();
            let first_line: String = s.lines().next()
                .unwrap_or("")
                .chars().take(80)
                .collect();
            let summary = format!("{lines} lines | {first_line}…");
            ToolOutput { summary, content: Some(s) }
        }
    }
}
```

`SUMMARY_THRESHOLD`: summary のみで十分な小さい出力の閾値。
具体値は調整するが、数百バイト程度を想定。

## Prune との関係

```
ツール実行
  → ToolOutput { summary, content }
  → Item::ToolResult { summary, content }    ← history に追加

    ─── 数ターン経過 ───

Prune（pre_llm_request フック）
  → Item::ToolResult { summary, content: None }  ← content を除去
```

Prune の実装は `content = None` にするだけ。

prunable トークン数の推定:
- `content.as_ref().map(|c| c.len() / 4).unwrap_or(0)`

## 巨大出力の扱い

フレームワークは巨大出力を特別扱いしない。
ツール側が自分で判断して対処する。

```
巨大な grep 結果 → ツールがファイルに書き出す
                  → summary: "grep: TODO in src/ — 128 hits"
                  → content: ファイルパス + ヒット数の内訳（見取り図）

巨大なファイル読み取り → ツールが部分読み取りを提案
                        → summary: "read_file: data.csv — 50,000 lines"
                        → content: 先頭 N 行 + 末尾 M 行
```

LLM が詳細を見たい場合は、read_file / grep 等の汎用ツールで
ファイルを直接参照する。専用の inspect ツールは不要。

## 削除対象（旧設計からの移行）

| モジュール | 理由 |
|---|---|
| `ToolOutput` enum（Inline/Stored） | struct に置換 |
| `Content` enum（Text/Structured） | 不要 |
| `auto_summarize` / `auto_summarize_text` / `auto_summarize_structured` | 不要 |
| `ToolOutputProcessor` trait | 不要 |
| `BlobOutputProcessor` | 不要 |
| `BlobStore` trait / `FsBlobStore` | 不要 |
| `inspect_tool.rs` | 不要 |
| Worker の `output_processor` フィールド | 不要 |
