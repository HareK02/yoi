# ツール実行結果のサイズ上限

## レビュー状態

初回レビュー実施済み。指摘事項と判断は [tool-output-limit.review.md](tool-output-limit.review.md) を参照。
指摘1（デフォルト不適用）・指摘2（truncate と interceptor の順序）の修正待ち。

## 背景

Pod のセッションで、Glob が `pattern:"*"` でプロジェクト全体を走査し、
約 125KB / 推定 70k トークン超の tool_result を返した結果、次ターンのリクエストが
組織レートリミット（30k input tokens/分）を単発で超過して永久に 429 で詰む事故が発生した。

一度肥大化した履歴は prune/compact が走る前に再送され続け、待っても抜けられない。
根本原因はツールが「呼ばれた通りの結果を素直に全部返す」こと。ツール自身の件数上限
（例: `Glob` の 1000 件）はバイト/トークン単位の上限ではないため機能しない。

ツール実行結果のサイズを LLM に投げる前に強制的にキャップし、LLM に
「検索範囲を絞れ」と促す必要がある。

## 単位について

理想はトークン単位での上限だが、既存の `pod::token_counter` は provider の
`UsageRecord` 実測値を基にした按分・外挿専用で、**未送信の単発ツール出力を
事前にトークン計測する手段は現時点で存在しない**。
ローカルトークナイザは精度・信頼性の理由で意図的に持たない方針。

そこで本チケットではバイト数ベースで上限をかける。UTF-8 のため
`str::len()` で O(1) に判定でき、`floor_char_boundary` を使えば文字境界で
安全に切断できる。将来 provider 実測値ベースのトークン上限に
置き換える余地は残す（マニフェストのキー名をそれに合わせる）。

## 要件

- **単一チョークポイントで全ツールに効く**: 個別ツールの実装を信用しない。
  Tool 実行境界（`llm-worker::worker::execute_tools` 内、`ToolResult::from_output`
  直後）で `ToolOutput.content` のバイト数を計測し、
  上限を超えていたら切り詰めてから履歴に積む。
- **マニフェストで設定可能**: デフォルトは 16KB（30k/分レートリミットに対して
  余裕を持った値）。プロジェクトごと・ツールごとに上書き可能。
- **切り詰め後は LLM が検知できる**: `content` 末尾に
  `[truncated: N bytes dropped, refine your query]` 形式の追記を入れ、
  LLM が自発的に絞り込みを試みるヒントにする。`ToolError` にはしない
  （エラーにすると LLM がリトライループに入りやすい）。
- **`summary` には手を入れない**: summary は常に短い 1-2 行で、上限に達しない前提。
- **`content` が `None` の場合はスキップ**: 計測・切り詰めの対象外。

## マニフェスト

```toml
[worker.tool_output]
# 全ツール共通の既定上限（バイト）。省略時 16384。
default_max_bytes = 16384

# ツールごとの上書き。ツール名は登録名（"Glob", "Read", ...）。
[worker.tool_output.per_tool]
Read = 32768     # Read は大きいファイルを意図的に返すので少し緩める
Grep = 8192
```

- `[worker.tool_output]` セクション自体は省略可能。省略時はデフォルト 16KB が全ツールに適用。
- `per_tool` も省略可能。
- 未知のツール名がマップに含まれていても manifest エラーにはしない（ログ警告のみ）。

## 実装方針（実装順序）

1. `manifest::WorkerManifest` に `tool_output: Option<ToolOutputLimits>` を追加。
   `ToolOutputLimits { default_max_bytes: usize, per_tool: HashMap<String, usize> }`。
2. 切り詰め関数を `llm-worker` 側に薄く追加。
   入力: `content: String`, `limit: usize`, `tool_name: &str`。
   `content.len() <= limit` ならそのまま返す。超えていれば
   `str::floor_char_boundary(limit - suffix.len())` で切って末尾に注記を追記。
3. `Worker` 生成時に `tool_output: Option<ToolOutputLimits>` を渡し、
   `execute_tools` の結果ループで `ToolResult::content` を in-place に書き換える。
4. 各ツール単体には本チケットでは手を入れない。上限を踏んだツールに対して
   後続の改善（Glob が `git_ignore` を尊重する等）は別チケットで扱う。

## 非ゴール

- **ツール固有の賢い縮退**（Glob が件数で、Read が行範囲で、など）は扱わない。
  まず一律上限で事故を止め、各ツールの自主制限は必要に応じて別チケットで追加する。
- **prompt caching の導入**や compaction 側の改善は扱わない。
  本チケットは「1 回のツール結果が履歴に載る前にキャップする」ことだけに集中する。
- **入力側（ツール引数）のサイズ制限**は扱わない。
- **トークン単位での上限**は扱わない。将来 provider 実測値ベースの
  オンライン・トークン推定が利用可能になった段階で、本チケットで入れた
  バイト上限をトークン上限に置き換えることを検討する。
