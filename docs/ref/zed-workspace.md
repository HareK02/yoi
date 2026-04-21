# Zed モノレポのワークスペース・クレート規則

[zed-industries/zed](https://github.com/zed-industries/zed) のソースコードを読み解くための、ワークスペース構成とクレート分割に関する規則のまとめ。

---

## 1. ワークスペース構成

### 単一ワークスペース

- リポジトリ直下の `Cargo.toml` が `[workspace]` を持ち、`crates/` 以下のすべてのクレート（200個超）を members として束ねる単一ワークスペース。
- `default-members = ["crates/zed"]` が指定されており、ルートで `cargo run` するとエディタ本体が起動する。
- `Cargo.lock` はルートに1つだけ。すべてのクレートで共有される。
- ルートに `rust-toolchain.toml` / `clippy.toml` / `rustfmt.toml` / `.cargo/` を置き、ワークスペース全体に共通設定を効かせる。

### 内部依存は `[workspace.dependencies]` に集約

ルート `Cargo.toml` に **すべての内部クレートを path 指定で列挙** する:

```toml
[workspace.dependencies]
acp_thread       = { path = "crates/acp_thread" }
action_log       = { path = "crates/action_log" }
agent            = { path = "crates/agent" }
agent_ui         = { path = "crates/agent_ui" }
anthropic        = { path = "crates/anthropic" }
# ... 200個以上続く
```

外部クレート（serde, tokio, フォーク版 calloop など）も同じ場所に集約され、バージョンや git rev を一元管理する。

### 各クレートは `.workspace = true` で参照

個々の `crates/<name>/Cargo.toml` ではバージョン番号もパスも書かない:

```toml
[dependencies]
gpui.workspace = true
project.workspace = true
serde.workspace = true
```

これにより:
- バージョン更新やフォーク差し替えがルート1ファイルで完結
- 同じ依存が複数バージョンに分裂する事故が起きない
- 新しいクレートの追加は「フォルダ作成 → ルートに1行追加 → 使う側で `name.workspace = true`」だけ

### `publish = false`

ワークスペース全体で `publish = false`。crates.io には公開されないため、`editor` や `project`、`language` のような一般名詞を平気で使える。

---

## 2. クレート命名規則

### 表記ルール

- **`snake_case` で統一**。ハイフンや CamelCase は使わない。
- **全部小文字**、略語も小文字（`lsp`, `rpc`, `acp`, `aws`, `ui`）。
- **ディレクトリ名 = `package.name` = `[workspace.dependencies]` のキー** で完全一致。grep しやすさを優先。

### 命名パターン

#### 1. コア基盤は1語の名詞

`zed` / `gpui` / `editor` / `project` / `workspace` / `language` / `theme` / `ui` / `lsp` / `rpc` / `audio` / `assets` / `askpass`

#### 2. 機能ファミリーは「共通プレフィックス + サフィックス」

```
agent / agent_ui / agent_ui_v2 / agent_settings / agent_servers
auto_update / auto_update_ui / auto_update_helper
assistant_text_thread / assistant_slash_command / assistant_slash_commands
acp_thread / acp_tools
```

サフィックスの慣習:

| サフィックス | 役割 |
|---|---|
| `_ui` | 機能のビュー/ウィジェット層（コアロジックとは分離） |
| `_settings` | 設定スキーマと読み込み |
| `_servers` | 外部プロセス連携アダプタ |
| `_helper` | 補助バイナリ |
| `_v2` | 既存版と並行する新実装の隔離 |
| `_tools` | デバッグ/開発者向けユーティリティ |

#### 3. 外部プロトコル/ベンダー連携はその名前そのまま

`anthropic` / `bedrock` / `aws_http_client` のように、相手のサービスや仕様の名前をそのまま使う。`acp_*` (Agent Client Protocol) のようにプロトコルの略号を頭に付けて系列化するのも同様。

---

## 3. クレート分割の方針

### `zed` クレートは薄いシェル

`crates/zed/` の中身は最小限:

- `main.rs` — エントリポイント、CLI 引数、シングルインスタンス処理、クラッシュハンドラ、パス初期化
- `zed.rs` — グローバル action ハンドラ登録、`initialize_workspace` でのパネル組み立て
- `build.rs` / `RELEASE_CHANNEL`

`app.run(...)` の中身は **200個以上のクレートの `init(cx)` を正しい順序で呼ぶだけ**:

```rust
settings::init(cx);
theme::init(cx);
client::init(&client, cx);
workspace::init(app_state.clone(), cx);
editor::init(cx);
project_panel::init(cx);
agent_ui::init(fs, client, prompt_builder, languages, cx);
git_ui::init(cx);
// ...
```

`zed` は依存グラフの頂点に立つ唯一のクレートで、機能の置き場所ではなく **配線盤 (wiring)** として存在する。**「どこに置くか迷ったら `zed` 以外のどこかに置く」** が鉄則。

### 境界を切る基準

#### ① レイヤー（下から上への単方向依存）

```
gpui                                       ← UI フレームワーク
  ↓
text / language / fs / rpc / settings /    ← ドメインの基本型
theme / ui
  ↓
project / lsp / git / terminal /           ← サービス層
multi_buffer
  ↓
editor / workspace                         ← 中核機能
  ↓
project_panel / git_ui / agent_ui /        ← 個別機能 (パネル/ビュー)
diagnostics / search / ...
  ↓
zed                                        ← 配線だけ
```

下のレイヤーは上のレイヤーを知らない。逆方向の拡張点は trait（`workspace::Item`、`workspace::Panel` など）として下層が公開し、上層が実装する。

#### ② ロジックと UI の分離

GPUI 依存を上層に閉じ込めるため、ロジックと UI を別クレートに切る:

| ロジック | UI |
|---|---|
| `agent` | `agent_ui` |
| `auto_update` | `auto_update_ui` |
| `git` | `git_ui` |
| `project` | `project_panel` |

UI 側はロジック側を知るが、ロジック側は UI 側を知らない。

#### ③ `Project` と `Workspace` の二分

Zed の設計で最も象徴的な分割:

- **`project`** — worktree、LSP、ファイルシステム、Git といったサービスを束ねる。ヘッドレスでも動く側。
- **`workspace`** — ペイン分割、ドック、パネル、ステータスバーといったウィンドウ表示を束ねる。GPUI に強く依存する側。

これにより `Project` をリモートマシンで走らせ `Workspace` をローカルで走らせる、というリモート開発が素直に成立する。

#### ④ trait 境界 + Fake 実装

下層の重要な抽象は trait で公開し、テストでは Fake を差し込む:

- `fs::Fs` ↔ `FakeFs`
- `git::GitRepository` ↔ Fake 実装
- `language::LanguageRegistry`
- `gpui::Element`、`workspace::Item`/`Panel`

trait が置かれているクレートがそのまま境界になる。「どこまでがプラグイン可能か」がクレート一覧から読み取れる。

#### ⑤ 外部世界との接点ごとにクレートを切る

新規追加で既存クレートを編集しなくて済むように、外部依存ごとに独立させる:

- `lsp` / `dap` / `rpc` (プロトコル層)
- `anthropic` / `bedrock` / `open_ai` (LLM プロバイダごと)
- `aws_http_client` (特定の HTTP クライアント実装)
- `acp_thread` / `acp_tools` (Agent Client Protocol)
- `extension_host` + `extension_api` (拡張機能 WASM サンドボックス境界)

#### ⑥ コンパイル時間最適化

頻繁に編集される `editor` のような大きいクレートを切り離すことで、並列ビルドとインクリメンタルコンパイルの効率を上げる。ルート `Cargo.toml` には単一ファイルクレートに `codegen-units = 1` を効かせる調整も含まれる。

---

## 4. 「core」クレートを作らない

Zed には `core` / `common` / `shared` / `zed_core` といったクレートが**意図的に存在しない**。「コア」になりそうなものは機能軸で水平に分割される:

| ありがちな "core" の中身 | Zed での置き場所 |
|---|---|
| 基本データ型 (Rope, Point, Anchor) | `text` |
| ファイルシステム抽象 | `fs` (`Fs` trait + `FakeFs`) |
| 設定の読み書き | `settings` |
| テーマ・色 | `theme` |
| 共通 UI コンポーネント | `ui` |
| 汎用ユーティリティ関数 | `util` |
| RPC/シリアライズ | `rpc` / `proto` |
| ロギング | `zlog` |
| 言語抽象 | `language` |

### `core` を作らない理由

1. **ビルドグラフの直列化を避ける** — `core` を作るとほぼ全クレートがそこに依存し、1行触るたびに数百クレートが再コンパイルされる。並列ビルドの利点が消える。
2. **"ゴミ捨て場" 化の防止** — `core` や `common` は置き場所に迷ったコードが流れ込む磁石になり、責務が肥大化して循環依存の温床になる。Zed は **「迷ったら新しいクレートを作る」** 方向に振る。
3. **依存方向のドキュメント化** — クレートを細かく分けると `Cargo.toml` を見るだけで何に依存し何に依存していないかが一覧できる。`core` があるとこの情報量がゼロになる。

### 例外的な `util`

`crates/util` は雑多な汎用ヘルパ（`ResultExt`, paths, debouncer 等）が入る "ややコアっぽい" クレートだが:

- 名前は `core` ではなく `util` (補助関数の入れ物の慣習)
- GPUI にも `editor` にも依存しない、完全な下層
- trait や中核データ型は置かない (それは `text` や `fs` の役目)

責務を「補助関数集」に限定することでゴミ捨て場化を防いでいる。

---

## 5. まとめ

Zed のクレート分割の優先順位:

1. **`zed` には何も入れない** — 機能は必ず別クレートに押し出し、`zed` は init 呼び出しと CLI と main だけを持つ
2. **下から上への単方向依存** — 上層が必要な拡張点は trait で下層に公開する
3. **ロジック / UI を分ける** — `xxx` と `xxx_ui` のペア、GPUI 依存を上層に閉じ込める
4. **`Project` と `Workspace` を分ける** — ヘッドレス/リモート実行可能性のため
5. **外部世界 (プロトコル・ベンダー・拡張) ごとにクレートを切る** — 新規追加を「ファイル追加だけ」で済ませる
6. **Fake が置けるところに trait を置く** — テスト境界 = アーキテクチャ境界
7. **`core` を作らない** — 万能箱の代わりに責務を限定した小さな箱を用意する

クレート数の多さは管理コストではなく、**境界を守るためのコスト払い**である。
