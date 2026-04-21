# リファレンス: ローカルファイル操作ツール仕様

Claude Code がエージェントとして使うファイル操作ツールの仕様。
Pod のツール設計の参考資料。

## Read

ファイルを読む。画像・PDF・Jupyter notebook にも対応。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `file_path` | string | 必須 | 絶対パス |
| `offset` | integer | 任意 | 読み開始行番号（0始まり） |
| `limit` | integer | 任意 | 読み取り行数（offset と併用） |
| `pages` | string | 任意 | PDF用ページ範囲 (例: `"1-5"`)。最大20ページ/回 |

- デフォルトで先頭から最大2000行
- 出力は行番号付き（1始まり）
- ディレクトリは読めない

## Write

ファイルを新規作成、または全体上書き。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `file_path` | string | 必須 | 絶対パス |
| `content` | string | 必須 | 書き込む内容全体 |

- 既存ファイルは上書き
- 既存ファイルを Write する前に Read が必要（未読だとエラー）

## Edit

既存ファイルの部分的な文字列置換。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `file_path` | string | 必須 | 絶対パス |
| `old_string` | string | 必須 | 置換対象（ファイル内で一意であること） |
| `new_string` | string | 必須 | 置換後（old_string と異なること） |
| `replace_all` | boolean | 任意 | `true` で全出現箇所を置換。デフォルト `false` |

- 事前に Read が必要（未読だとエラー）
- `old_string` がファイル内で一意でないとエラー（`replace_all: true` 除く）

## Glob

ファイルパターン検索。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `pattern` | string | 必須 | glob パターン (例: `"**/*.rs"`) |
| `path` | string | 任意 | 検索ディレクトリ。省略時はカレント |

- 結果は更新日時順ソート

## Grep

ファイル内容の正規表現検索（ripgrep ベース）。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `pattern` | string | 必須 | 正規表現パターン |
| `path` | string | 任意 | 検索対象。省略時はカレント |
| `glob` | string | 任意 | ファイルフィルタ (例: `"*.rs"`) |
| `type` | string | 任意 | ファイル種別 (例: `"rust"`, `"py"`) |
| `output_mode` | enum | 任意 | `"files_with_matches"` (デフォルト) / `"content"` / `"count"` |
| `-n` | boolean | 任意 | 行番号表示 (content モード、デフォルト `true`) |
| `-i` | boolean | 任意 | 大文字小文字無視 |
| `-A` | number | 任意 | マッチ後の行数 |
| `-B` | number | 任意 | マッチ前の行数 |
| `-C` | number | 任意 | マッチ前後の行数 |
| `multiline` | boolean | 任意 | 複数行マッチ。デフォルト `false` |
| `head_limit` | number | 任意 | 出力件数制限。デフォルト 250 |
| `offset` | number | 任意 | 先頭N件スキップ |

## Bash

シェルコマンド実行。

| パラメータ | 型 | 必須 | 説明 |
|---|---|---|---|
| `command` | string | 必須 | 実行するコマンド |
| `description` | string | 任意 | コマンドの説明 |
| `timeout` | number | 任意 | タイムアウト ms (最大 600000、デフォルト 120000) |
| `run_in_background` | boolean | 任意 | バックグラウンド実行 |

- 作業ディレクトリはコマンド間で永続
- シェル状態（変数・エイリアス）は永続しない

## 設計上の特徴

- **パスは全て絶対パス**
- **Read → Edit/Write の順序制約**: 未読ファイルの編集を防ぐ安全弁
- **専用ツール優先**: cat/grep/find/sed の代わりに専用ツールを使う
- **冪等性**: Edit は差分ベース、Write は全体上書き
- **ワークフロー**: Glob/Grep で探索 → Read で確認 → Edit で変更
