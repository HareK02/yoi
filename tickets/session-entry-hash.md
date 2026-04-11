# セッションエントリのハッシュチェーン

## 背景

複数の Pod が同じ Session を読み込んで作業を進めるケースがある。現状の設計では、同一セッションファイルへの同時書き込みがコンフリクトを起こす。また `fork_at` のエントリ指定が `usize`（インデックス）であるため、元セッションにエントリが追加されると同じインデックスが別の内容を指してしまう。

## やること

- 各 `LogEntry` に `prev_hash: Option<EntryHash>` を追加（先頭エントリは `None`）
- `EntryHash` は `sha256(prev_hash + serialized_content)` で算出
- `Session` がロード時に head hash（末尾エントリのハッシュ）を保持
- `Session::append` 時にファイル末尾のハッシュと保持している head hash を比較
  - 一致 → 通常 append、head hash を更新
  - 不一致 → auto-fork（新 SessionId で分岐）
- `fork_at` の引数を `usize` → `EntryHash` に変更

## アドレッシング

- **SessionId (UUID v7)** → どのセッション（ファイル）か
- **EntryHash** → そのセッション内のどの時点か

## 判断根拠

- ファイルサイズやエントリ数による変更検知は同じ長さの別内容を区別できず信頼性が低い
- ハッシュチェーンなら暗号的に一意であり、ログの整合性検証も副産物として得られる
- ストレージレイアウト（JSONL）は変更不要。LogEntry の構造変更だけで済む
