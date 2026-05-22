# llm-worker-persistence

Worker のセッション永続化を提供するクレート。追記専用の JSONL ログとして状態遷移を記録し、ログの再生によってセッションを完全に復元する。大きなツール出力は Blob ストアに分離保存する。

## 公開型

### セッション

- `Session<C, St>` — Worker をラップした永続化セッション（`run()`, `resume()`, `fork()`, `fork_at()`）
- `SessionId` — UUID v7 によるセッション識別子
- `SessionConfig` — 永続化設定（イベントトレース記録の有無）

### ストア

- `Store` トレイト — 永続化バックエンド抽象（`append`, `read_all`, `list_sessions`）
- `FsStore` — ファイルシステム上の JSONL ストア実装
- `BlobStore` トレイト — Blob ストレージ抽象（`store`, `load`）
- `FsBlobStore` — ファイルシステム上の Blob ストア実装
- `BlobOutputProcessor` — ToolOutputProcessor 実装（小さい出力はインライン、大きい出力は Blob 保存）

### ログ

- `LogEntry` — セッションログのエントリ型（`SegmentStart`, `UserInput`, `AssistantItem`, `ToolResult`, `SystemItem`, `TurnEnd` など）
- `RestoredState` — ログ再生で復元された状態
- `collect_state()` — ログエントリ列から状態を復元する関数

### ツール

- `InspectTool` — Blob 内容を取得する組み込みツール（行範囲・配列スライス・キー指定セレクタ対応）
