# 組み込みツール実装 (tools クレート)

## 背景

リファレンス仕様 (`docs/ref/reference-tool-spec.md`) に基づき、エージェントの基本操作となる
ファイル操作ツール群と Bash ツールを実装する。

新規 `tools` クレートを作成し、llm-worker のツール基盤（`Tool` trait, `ToolServer`）の上に
具体的なツール実装を載せる。

## 実装対象

| ツール | 概要 | Scope |
|--------|------|-------|
| Read | ファイル読み取り（offset/limit対応） | 制限なし |
| Write | ファイル新規作成/全体上書き | ScopedFs で書き込み制限 |
| Edit | 部分文字列置換（一意性チェック, replace_all） | ScopedFs で書き込み制限 |
| Glob | glob パターンによるファイル検索 | 制限なし |
| Grep | 正規表現によるファイル内容検索 | 制限なし |
| Bash | シェルコマンド実行 | Permission 層で制御（別チケット） |

## ScopedFs

ファイル操作の Scope 境界を構造的に保証するラッパー。

```rust
pub struct ScopedFs {
    scope: Scope,
}

impl ScopedFs {
    // Read 系 — 制限なし
    fn read(&self, path: &Path) -> io::Result<String>;
    fn read_lines(&self, path: &Path, offset: usize, limit: usize) -> io::Result<String>;
    fn glob(&self, pattern: &str, base: Option<&Path>) -> Vec<PathBuf>;
    fn grep(&self, pattern: &str, path: Option<&Path>, opts: GrepOpts) -> GrepResult;

    // Write 系 — Scope チェック付き
    fn write(&self, path: &Path, content: &str) -> io::Result<()>;
    fn replace(&self, path: &Path, old: &str, new: &str, all: bool) -> io::Result<()>;
}
```

- 各ファイル操作ツール（Read/Write/Edit/Glob/Grep）は ScopedFs を通してのみ fs に触る
- Bash は子プロセスが直接 fs を触るため ScopedFs では守れない → Permission チケットの deny/allow ルールで対応
- 既存の `manifest::Scope` を ScopedFs 内部で利用。Scope 自体は manifest に残す

## ツール実装パターン

```rust
// tools クレートで実装
pub struct ReadTool {
    fs: ScopedFs,
}

impl Tool for ReadTool {
    async fn execute(&self, input: &str) -> Result<String, ToolError> {
        let params: ReadParams = serde_json::from_str(input)?;
        self.fs.read_lines(&params.file_path, params.offset, params.limit)
            .map_err(ToolError::from)
    }
}

// ToolDefinition ファクトリで登録
pub fn read_tool(fs: ScopedFs) -> ToolDefinition { ... }
```

全ツールのファクトリをまとめた登録関数を公開:

```rust
pub fn builtin_tools(fs: ScopedFs) -> Vec<ToolDefinition> {
    vec![read_tool(fs.clone()), write_tool(fs.clone()), ...]
}
```

## Bash ツール

- コマンド実行、タイムアウト、バックグラウンド実行をサポート
- 作業ディレクトリは永続（ツール内部で状態保持）
- Scope による保護は不可 → `PreToolCall` Hook + Permission ルールで制御
- Permission チケット未実装の間は、ツール自体は登録可能だが制約なしで動作

## 依存関係

- `tools` クレートは `llm-worker`（Tool trait）と `manifest`（Scope）に依存
- Permission による Bash 制御 → [permission-extension-point.md](permission-extension-point.md)

## 将来の拡張

- ScopedFs をスクリプティング言語ランタイムに公開し、ユーザー定義ツールからも同じ Scope 境界で fs 操作を可能にする
