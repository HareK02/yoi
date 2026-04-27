# ホームディレクトリ配下のディレクトリ整理

## 背景

現状、Insomnia のホーム配下のファイルは 2 つのツリーに分かれていて、規約が一貫していない:

- `$XDG_CONFIG_HOME/insomnia/` (fallback `~/.config/insomnia/`)
  - `manifest.toml` (user manifest)
  - `providers.toml` / `models.toml` (catalog user override)
  - `prompts/` / `prompts.toml` (user prompts)
- `~/.insomnia/`
  - `sessions/` (session store)
  - `run/` (XDG_RUNTIME_DIR 不在時の socket fallback)

実態としては「config は XDG 尊重、data / runtime は `~/.insomnia/`」というハイブリッド構成だが、

- ロジックが 11 箇所に分散していて (`pod/src/main.rs` / `pod/src/runtime/dir.rs` / `pod/src/runtime/scope_lock.rs` / `tui/src/main.rs` / `manifest/src/cascade.rs` / `provider/src/catalog.rs` / `pod/src/factory.rs` 等)、`default_runtime_dir` (main.rs) と `default_base` (dir.rs) のように完全重複しているものもある
- すべて `std::env::var()` の手作業実装で、XDG 解決ライブラリ (`dirs` / `directories` 等) は未導入
- どこに何を置くかが docs にも module comment にも明記されていない
- テストでパス上書きの手段が統一されておらず、`unsafe std::env::set_var` を多用、`scope_lock.rs` には並行テスト用 `ENV_LOCK: Mutex` まで存在する
- 既存の override 環境変数は `INSOMNIA_SCOPE_LOCK` （scope.lock 単体）しかなく、テスト/開発用に統一された口がない

新しい機能（モデル設定 wizard 等）を足す前に、レイアウトの規約と解決ロジックを 1 箇所に集約する。

## 方針

### レイアウト: 案 C （ハイブリッド、現状追認）

config 系は XDG、data / runtime 系は `~/.insomnia/` 配下を維持。`XDG_CONFIG_HOME` / `XDG_RUNTIME_DIR` を既に尊重しており、現状の動作を壊さない:

```
$XDG_CONFIG_HOME/insomnia/    # 人が編集する設定 (fallback ~/.config/insomnia/)
  manifest.toml
  providers.toml
  models.toml
  prompts/
  prompts.toml
~/.insomnia/                  # プログラムが書くデータ・ランタイム
  sessions/
  run/                        # XDG_RUNTIME_DIR があればそちら優先
```

`$XDG_DATA_HOME` / `$XDG_STATE_HOME` には対応しない（必要になったら別途検討）。

### ロジック集約: paths モジュール

パス解決を 1 箇所に集約する。crate は新設せず、`crates/manifest` 配下に `paths` module を追加して全 crate からそれを参照する形にする (manifest crate は既に pod / tui / provider から依存されている)。

集約対象:

- `pod/src/main.rs:default_store_dir` (sessions)
- `pod/src/main.rs:default_runtime_dir` と `pod/src/runtime/dir.rs:default_base` (重複)
- `pod/src/runtime/scope_lock.rs:default_lock_path`
- `tui/src/main.rs:resolve_socket`
- `manifest/src/cascade.rs:user_manifest_path`
- `provider/src/catalog.rs:user_override_path`
- `pod/src/factory.rs` の user/project prompts dir 導出

`paths` module は以下のような API を提供する想定（命名は実装時に確定）:

- `config_dir()` → `$XDG_CONFIG_HOME/insomnia` or `~/.config/insomnia`
- `data_dir()` → `~/.insomnia`
- `runtime_dir()` → `$XDG_RUNTIME_DIR/insomnia` or `~/.insomnia/run`
- 各 well-known file への getter (`user_manifest_path()`, `sessions_dir()`, `scope_lock_path()`, `socket_path(pod_name)` 等)

### 環境変数の整理

現状散らばっている override を整理する:

| 変数 | 現状 | 新方針 |
|---|---|---|
| `INSOMNIA_HOME` | 無し | **新設**。設定すると config_dir / data_dir / runtime_dir すべての base になる（テスト・開発・ポータブル運用向けの統一 override） |
| `INSOMNIA_CONFIG_DIR` | 無し | **新設**。config_dir の個別 override (INSOMNIA_HOME より優先) |
| `INSOMNIA_DATA_DIR` | 無し | **新設**。data_dir の個別 override |
| `INSOMNIA_RUNTIME_DIR` | 無し | **新設**。runtime_dir の個別 override |
| `INSOMNIA_SCOPE_LOCK` | scope.lock の path 直接指定 (テスト用) | **廃止**。`INSOMNIA_RUNTIME_DIR` で代替 |
| `INSOMNIA_POD_COMMAND` | 子 Pod 起動コマンド | **存続**（パス系ではない） |
| `INSOMNIA_API_KEY_*` | scheme 別の認証 env | **存続**（パス系ではない） |
| `XDG_CONFIG_HOME` | 尊重 | 引き続き尊重 (INSOMNIA_* 系より低優先) |
| `XDG_RUNTIME_DIR` | 尊重 | 引き続き尊重 (INSOMNIA_* 系より低優先) |

優先順位: `INSOMNIA_<KIND>_DIR` > `INSOMNIA_HOME` > `XDG_*` > 既定 (`~/.config/insomnia` または `~/.insomnia`)

`INSOMNIA_HOME` 導入によりテストでの `unsafe std::env::set_var("XDG_CONFIG_HOME", ...)` を `INSOMNIA_HOME=tempdir` で置き換えられるようになる（`scope_lock.rs:980` の `ENV_LOCK` Mutex も整理可）。

### マイ採用しないション

レイアウトは現状追認なので、既存ユーザの自動移行は不要。`INSOMNIA_SCOPE_LOCK` の廃止のみ、削除時に release notes で告知する（テスト用なので外部利用想定はほぼ無い）。

## 完了条件

- `crates/manifest/src/paths.rs` (仮) に config / data / runtime の各責務と解決ロジックがまとめられ、module コメントで「どこに何が置かれるか」が明記されている
- 既存の 11 箇所のパス解決が新 module の API 呼び出しに置き換わっている（特に `default_runtime_dir` と `default_base` の重複が解消されている）
- `INSOMNIA_HOME` / `INSOMNIA_CONFIG_DIR` / `INSOMNIA_DATA_DIR` / `INSOMNIA_RUNTIME_DIR` の override が動作し、優先順位が決まっている
- `INSOMNIA_SCOPE_LOCK` が削除され、テストは `INSOMNIA_HOME` か `INSOMNIA_RUNTIME_DIR` 経由に書き換わっている
- `unsafe std::env::set_var` を使っているテスト箇所が、新 override に対応した最小限の数まで減っている
- pod の起動と tui の spawn flow が引き続き動作する

## 範囲外

- Windows / macOS のネイティブ規約（`%APPDATA%` / `~/Library/Application Support` 等）への対応。本チケットは Linux / 一般的な Unix 想定
- `$XDG_DATA_HOME` / `$XDG_STATE_HOME` への対応（必要になったら別途）
- TUI 側で setup wizard が「どこに何を書く」表示するかの UX。本チケットの結論を `tickets/tui-user-model-setup.md` が前提として使う

## 後続チケット

- `tickets/tui-user-model-setup.md`: 本チケットで確定したレイアウトに従って user manifest を書き込む wizard を実装する

## Review

- 状態: Approve
- レビュー詳細: [./home-dir-layout.review.md](./home-dir-layout.review.md)
- 日付: 2026-04-27
