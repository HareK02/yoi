# TUI: ユーザーマニフェストのモデル設定 wizard

## 背景

spawn UI（`tickets/tui-pod-spawn-ui.md`）が `[model]` を user / project の cascade レイヤから取る前提なので、初回起動のユーザーは事前に `~/.config/insomnia/manifest.toml` を手で書く必要がある。catalog（`crates/provider/src/catalog.rs`）に provider / model の一覧と `AuthHint`（API key の env 名や Codex OAuth 等の認証方式）が既に揃っているので、これを使って TUI 内で対話的にセットアップできるようにする。

`provider::catalog` の公開 API:

- `load_providers() -> Vec<ProviderEntry>`: builtin + user override マージ済み
- `load_models() -> Vec<ModelEntry>`: 同上
- `ProviderEntry`: `id` / `display_name` / `scheme` / `auth_hint` 等
- `ModelEntry`: `id` / `provider` / `capability`
- `AuthHint`: `None` / `ApiKey { env: Option<String> }` / `CodexOAuth`

これらが「UI で何を選ばせ、何を聞くか」を直接ガイドしてくれる構造になっている。

## 要件

### 起動経路

- 専用サブコマンド `tui setup-model`（仮）として alt-screen TUI で起動する
- 引数なしで叩くと既存の spawn flow に入るので、それとは別の入口

### Wizard フロー

1. **provider 選択**: `load_providers()` の結果をリスト表示。`display_name` を見せ、上下キー + Enter で選択
2. **model 選択**: 選んだ provider の `id` で `load_models()` をフィルタしてリスト表示。1 つだけならスキップ可
3. **認証情報入力**: 選んだ provider の `auth_hint` で分岐
   - `None` → スキップ
   - `ApiKey { env: Some(name) }` → 「環境変数 `<name>` を使う」または「key ファイルパスを入力」を選ばせる
   - `ApiKey { env: None }` → key ファイルパス入力（絶対パス推奨、ホーム展開はする）
   - `CodexOAuth` → 「`codex login` で OAuth を済ませてください」案内 + `~/.codex/auth.json` の存在チェック
4. **確認画面**: 書き込み内容のプレビュー（生成される TOML）を表示、Enter で確定 / Esc でキャンセル
5. **書き込み**: `~/.config/insomnia/manifest.toml`（または `$XDG_CONFIG_HOME` 配下、`manifest::user_manifest_path()`）に `[model]` を書く

### 書き込みフォーマット

catalog 由来なので `ref` 形式を採用する:

```toml
[model]
ref = "<provider_id>/<model_id>"

[model.auth]
kind = "api_key"
file = "/abs/path/to/key"
```

`AuthHint::None` の場合は `[model.auth]` を省く。`CodexOAuth` の場合は `kind = "codex_oauth"`。

### 既存ファイルの扱い

`~/.config/insomnia/manifest.toml` が既に存在する場合:

- `[model]` セクションが無い → 末尾に追加
- `[model]` セクションが既にある → 上書き確認を出す（既存の値をプレビュー表示してから）
- ファイル全体が壊れた TOML → エラー表示してキャンセル

### キャンセル / エラー経路

- どのステップでも Esc / Ctrl-C で抜けられる。書き込み前ならファイルは触らない
- catalog 読み込み失敗 / ファイル書き込み失敗は alt-screen 内でエラー表示してから終了

## 設計で決めること

- **API key ファイルパスの入力 UX**: テキスト入力欄でフリーフォーム、補完なしで良いか、`~` / `$HOME` 展開するか
- **環境変数で済ませる選択肢の見せ方**: ApiKey で env 指定がある場合、デフォルト「env を使う」かデフォルト「key ファイルを使う」か
- **多 provider / 多 model 時の選択 UI**: シンプルな縦リストか、検索フィルタ付きか
- **既存 `[model]` の上書き確認の粒度**: TOML 全体 diff か、変わるキーだけハイライトか

## 完了条件

- `tui setup-model` サブコマンドで wizard が起動する
- catalog から provider / model 一覧を取って表示・選択できる
- `AuthHint` の各バリアントに対応した入力 UI が動く
- 確定すると `~/.config/insomnia/manifest.toml` に `[model]` が書き込まれる
- 既存ファイルの `[model]` 上書き時は確認が出る
- セットアップ後に spawn flow（引数なし `tui` 起動）が model resolve エラー無しで Pod を spawn できる

## 範囲外

- catalog 自体の編集（新規 provider / model の追加）UI。`providers.toml` / `models.toml` の手書き運用は維持
- 複数モデル設定（`[compaction.model]` 等）の wizard 化
- project manifest (`.insomnia/manifest.toml`) への書き込み。本チケットは user 層のみ
- spawn flow からの自動誘導（model 不在検出時に「`m` で setup wizard を起動」分岐）。本チケット完了後に spawn UI 側で別途検討
