# ToolOutput.referenced_files: ツールが参照したファイルの追跡

## 背景

Compact 実行時、「タスク続行に必要なファイル」を compact worker に提示するため
のデフォルトリファレンス（過去に読み書きされたファイル一覧）が必要。
builtin-tools は実装済みだが、Pod 側でファイル操作を追跡する口がない。

ツール名ベースのヒューリスティック検出（"read" / "edit" / "write" の名前で判別）は
脆弱でユーザー定義ツールに対応できない。各ツールが自己申告する形にする。

## 方針

`ToolOutput` に optional な `referenced_files: Vec<PathBuf>` を追加し、ツール自身が
「この実行で触ったファイル」を申告する。Pod の `HookInterceptor::post_tool_call` で
観察し、Pod 内の LRU 的な履歴に積む。Compact 時はその履歴から先頭 N 件を compact
worker のデフォルトリファレンスとして渡す。

## 実装

### llm-worker: ToolOutput 拡張

```rust
pub struct ToolOutput {
    pub summary: String,
    pub content: Option<String>,
    pub referenced_files: Vec<PathBuf>,  // NEW. 空なら未申告
}
```

- `From<String>` 実装では空の Vec を入れる
- 既存のツール実装は変更不要（デフォルトで空）
- `ToolResultInfo` （`post_tool_call` Hook が受ける型）にも伝播させて Pod から見えるようにする

### builtin-tools: 各ツールで埋める

既に実装済みの以下のツールに `referenced_files` を埋める変更を入れる:

| ツール | 申告するファイル |
|--------|------------------|
| Read   | 読んだファイルパス |
| Write  | 書いたファイルパス |
| Edit   | 編集したファイルパス |
| Glob   | （積極的には埋めない — マッチしたファイル全部は多すぎる） |
| Grep   | （同上） |
| Bash   | （不可能。空のまま） |

Glob/Grep は「発見」しかしていないので referenced とは扱わない。

### Pod: 追跡と取り出し

`Pod` 内に履歴バッファ:

```rust
referenced_files: VecDeque<PathBuf>,  // 最新 N 件, 重複排除
```

- `HookInterceptor::post_tool_call` で `ToolResultInfo.referenced_files` を受け取り、
  既存エントリを先頭に移動 or 新規追加（LRU 的な挙動）
- 容量上限（例: 20）を超えたら末尾を落とす
- Compact 開始時に先頭 5 件を取り出して compact worker に渡す

## 設計ポイント

- **ツールが自己申告**: 外部から名前で判別しない。拡張性のため
- **Vec<PathBuf> なので 0〜複数件**: 複数ファイルに触るツール (例: 大量置換) にも対応
- **空 Vec 許容**: Bash や Glob のような「追跡不能 or 不適切」なツールはそのまま空
- **Pod が LRU バッファを持つ**: Compact 時の抽出を O(1) に近づける。Worker 層は関知しない

## 依存

- builtin-tools が実装済み前提（チケット: [builtin-tools.md](builtin-tools.md) 完了後）

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — デフォルトリファレンスの抽出がこのフィールドに依存
