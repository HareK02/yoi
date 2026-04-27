# ホームディレクトリ配下のディレクトリ整理

## 背景

現状、Insomnia のホーム配下のファイルは 2 つのツリーに分かれていて、規約が一貫していない:

- `$XDG_CONFIG_HOME/insomnia/` (fallback `~/.config/insomnia/`)
  - `manifest.toml` (user manifest)
  - `providers.toml` / `models.toml` (catalog user override、`crates/provider/src/catalog.rs`)
  - `prompts/` / `prompts.toml` (user prompts、`crates/pod/src/prompt/loader.rs`)
- `~/.insomnia/`
  - `sessions/` (session store、`crates/pod/src/main.rs:default_store_dir`)
  - `run/` (XDG_RUNTIME_DIR 不在時の socket fallback、`crates/pod/src/runtime/dir.rs:default_runtime_base`)

config 系は XDG に従っているが、data / runtime 系は `~/.insomnia/` 直下に独自に積まれている。新しい機能（モデル設定 wizard 等）を足す前に、どこに何を置くかをはっきりさせたい。

## 方針候補

### 案 A: XDG 完全準拠

各ディレクトリを XDG Base Directory Specification に分ける:

| 種別 | XDG 変数 | fallback | 内容 |
|---|---|---|---|
| config | `$XDG_CONFIG_HOME/insomnia/` | `~/.config/insomnia/` | manifest / catalog / prompts |
| data | `$XDG_DATA_HOME/insomnia/` | `~/.local/share/insomnia/` | sessions |
| state | `$XDG_STATE_HOME/insomnia/` | `~/.local/state/insomnia/` | logs (将来) |
| runtime | `$XDG_RUNTIME_DIR/insomnia/` | `~/.cache/insomnia/run/` 等 | socket / scope lock |

**Pros**: 標準に従う。バックアップ対象（data）と捨てて良いもの（runtime）が分離する。
**Cons**: ディレクトリが 3〜4 箇所に分散して把握しにくい。fallback 経路の選択も複雑。

### 案 B: `~/.insomnia/` 一元化

`~/.insomnia/` 配下に config も data も runtime も全部置く:

```
~/.insomnia/
  manifest.toml          # 旧 ~/.config/insomnia/manifest.toml
  providers.toml         # 旧 ~/.config/insomnia/providers.toml
  models.toml
  prompts/
  sessions/
  run/                   # XDG_RUNTIME_DIR があればそちら、無ければここ
```

XDG 系の env が設定されていればそれを優先、無ければ `~/.insomnia/` で完結。

**Pros**: 全部同じ場所、ファイル探索 / バックアップ / 移行が楽。`~/.config/insomnia/` を見たときに「あれ、設定ここだっけ？」とならない。
**Cons**: XDG 規約から外れる。Linux ユーザの期待とずれる可能性。

### 案 C: ハイブリッド（現状の整理）

config だけ XDG、data / runtime は `~/.insomnia/` 配下を維持。位置は変えず命名と responsibility を整理:

```
$XDG_CONFIG_HOME/insomnia/   # 永続的な設定（人間が手で書く / 編集する）
  manifest.toml
  providers.toml
  models.toml
  prompts/
~/.insomnia/                  # ランタイム & データ（プログラム生成・状態）
  sessions/
  run/
  cache/                     # 将来用
```

**Pros**: 「人が編集するもの」と「プロセスが書くもの」が物理的に分かれる。現状からの移行が小さい。
**Cons**: XDG 半準拠なので、XDG_DATA_HOME を尊重したいユーザは別途設定が必要。

## 設計で決めること

- **どの案を採用するか** (A / B / C のどれか、または別案)
- **環境変数による override**: `INSOMNIA_HOME` / `INSOMNIA_CONFIG_DIR` / `INSOMNIA_DATA_DIR` / `INSOMNIA_RUNTIME_DIR` のような上書き口を入れるか
- **マイ採用しないション**: 既存の `~/.insomnia/sessions/` / `~/.config/insomnia/manifest.toml` 配置を持つユーザの data を新パスに引き継ぐか、初回起動時にメッセージだけ出して移動はユーザに任せるか
- **socket / scope_lock の扱い**: runtime はマシン再起動で消えて良いものなので XDG_RUNTIME_DIR 優先で良いか、`~/.insomnia/run/` が常に唯一の置き場で良いか
- **prompts / catalog override の置き場**: config に近いので config 側でほぼ確定だが、project ローカル prompts (`<project>/.insomnia/prompts/`) との対応関係を整理する

## 完了条件

- 各ディレクトリの責務（config / data / state / runtime）と置き場が docs か module レベル comment に明記されている
- pod / tui 各クレートの該当箇所（`pod/src/main.rs` / `pod/src/runtime/dir.rs` / `pod/src/runtime/scope_lock.rs` / `manifest/src/cascade.rs` / `provider/src/catalog.rs` / `pod/src/prompt/loader.rs`）が新規約に揃っている
- 既存ユーザ向けのマイ採用しないションパスが決まっている（自動 / 手動 / 何もしないのいずれか明文化）
- 新しいレイアウトで pod の起動と tui の spawn flow が引き続き動く

## 範囲外

- Windows / macOS のネイティブ規約（`%APPDATA%` / `~/Library/Application Support` 等）への対応。本チケットは Linux / 一般的な Unix 想定
- TUI 側で setup wizard が「どこに何を書く」表示するかの UX。本チケットの結論を `tickets/tui-user-model-setup.md` が前提として使う

## 後続チケット

- `tickets/tui-user-model-setup.md`: 本チケットで確定したレイアウトに従って user manifest を書き込む wizard を実装する
