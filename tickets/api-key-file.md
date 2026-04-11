# api_key_file: ファイルパスによるAPIキー解決

## 背景

現状、APIキーの取得手段は `api_key_env`（環境変数名の指定）のみ。
永続化やインタラクティブ入力の仕組みがなく、キー管理をユーザーのシェル環境に完全依存している。

## やること

マニフェストの `ProviderConfig` に `api_key_file: Option<PathBuf>` を追加し、ファイルからAPIキーを読み取れるようにする。

### マニフェスト

```toml
[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_file = "~/.config/insomnia/keys/anthropic"
```

- ファイルにはキーのみを記載（読み込み時に trim）
- `~` 展開が必要
- 相対パスはマニフェストファイルの位置基準

### api_key_env との関係

- 排他。両方指定されたらエラー
- Ollama は両方不要のまま

### 変更箇所

1. **manifest**: `ProviderConfig` に `api_key_file: Option<PathBuf>` を追加
2. **provider**: `build_client()` でファイル読み取りロジックを追加。排他バリデーション
3. **provider**: `ProviderError` にキー不在を明示するバリアント追加（将来の TUI フォールバック用）

### 暗号化について

現段階では扱わない。ファイルパーミッション（0600）で十分。
将来エンドユーザー向けに暗号化が必要になった場合、provider の手前に復号レイヤーを挟む形で対応できる。`api_key_file` の設計自体は変更不要。

### 将来の拡張

- TUI サブコマンド（`insomnia key set anthropic` 等）がこのファイルに書き込むラッパーになる
- `api_key_cmd`（コマンド実行によるキー取得）は `api_key_file` で不足が生じた時点で検討
