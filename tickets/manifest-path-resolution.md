# Manifest のパス解決: cwd ベース + manifest ファイル相対

レビュー中: [manifest-path-resolution.review.md](manifest-path-resolution.review.md)

## 背景

現状 manifest 内のパス（`pod.pwd` / `provider.api_key_file` / `scope.allow.target` / `scope.deny.target` / `compaction.provider.api_key_file`）は全て絶対必須で、相対パスは `ResolveError::RelativePath` で弾かれる。

これは 4 層（builtin / user / project / overlay）のカスケードで「相対の基準点が層ごとに違う」曖昧さを避けるための制約だったが、次の点で歪みを生んでいる：

- `pod.pwd` フィールドが Unix 慣習（cwd はプロセス状態、config には書かない）と乖離
- project manifest で `scope.allow = [{ target = "<repo>" }]` のような絶対パスを強いられ、プロジェクトをどこに置いても動くはずの設定が壊れる
- user manifest で `api_key_file = "~/.config/insomnia/keys/anthropic"` が書けない（`~` 展開もしていない）

cargo / pyproject / npm などに倣い「相対パスは manifest ファイルの位置基準」に切り替える。合わせて `pod.pwd` を廃止し、プロセスの cwd を使う。

## 新しい解決規則

- `pod.pwd` フィールドは削除。Pod の作業ディレクトリ = プロセスの cwd
- 相対パスの基準は層ごとに決める
  - user manifest (`~/.config/insomnia/manifest.toml`) の相対 = そのファイルの親ディレクトリ
  - project manifest (`<project>/.insomnia/manifest.toml`) の相対 = **プロジェクトルート**（`.insomnia/` の親）。`target = "."` がワークスペース全体を指すように
- overlay（インライン TOML、ファイル位置なし）の相対パスは **プロセスの cwd** 基準
- builtin 層には manifest を埋め込んでいないので対象外

## 解決のタイミング

各層を**マージする前に絶対化**する。層をまたいだ相対パス合成は行わない。

```
user.toml (partial) → resolve_paths(base=user_dir) → absolute partial
                                                      │
project.toml (partial) → resolve_paths(base=prj_dir) ─┤
                                                      │── merge → PodManifestConfig
overlay (partial) → resolve_paths(base=cwd) ──────────┤
                                                      │
builtin defaults → ────────────────────────────────── ┘
```

`TryFrom<PodManifestConfig>` の時点では全パスが絶対になっているので、`ensure_absolute` は不変条件のチェックとしてのみ残す。

## 影響範囲

### `crates/manifest`

- `PodManifestConfig` から `pod.pwd` を削除
- 各 partial config を「ベースパス付き」で解決するヘルパーを追加（関数シグネチャ案: `fn resolve_partial(cfg: PodManifestConfigPartial, base: &Path) -> PodManifestConfigPartial`）
- 対象フィールド: `provider.api_key_file` / `scope.allow.target` / `scope.deny.target` / `compaction.provider.api_key_file`

### `crates/pod/src/factory.rs`

- `with_user_manifest` / `with_project_manifest_from` は渡された manifest ファイルの親ディレクトリを base として保存、解決時に使う
- `with_overlay_toml` / `with_overlay_config` はプロセス cwd を base として使う
- マージ順は現状のまま（overlay が最優先）

### `crates/pod/src/pod.rs` 他

- `pod.pwd` を参照している箇所を `std::env::current_dir()` に置き換え
- `Pod::from_manifest` / `from_manifest_spawned` のシグネチャから pwd 関連を削除

### `crates/pod/src/spawn_pod.rs`

- overlay TOML 構築から `pod.pwd` を消す
- 子プロセス起動時に `Command::current_dir(spawner_pwd)` で cwd を明示
  （現状の「spawner の pwd を子に引き継ぐ」挙動を維持するため）
- 将来、LLM が子の cwd を明示的に指定したくなったら `SpawnPod` の入力に `cwd` を足す（本チケット範囲外）

### `crates/pod/src/main.rs`

- `--pwd` フラグは削除（cwd が代替）
- 起動スクリプトや TUI 側で `cd` してから `pod` を起動する運用に変更

## 完了条件

- `pod.pwd` フィールドの削除
- 各層のパスが manifest ファイル基準（overlay は cwd 基準）で解決される
- マージは絶対化後の値で行う
- 既存テストが通る / 相対パスを使った manifest で起動可能
- `api_key_file = "keys/anthropic"` が user manifest 内で動作
- project manifest で `scope.allow = [{ target = "." }]` が動作

## 範囲外

- `~` 展開（`dirs::home_dir()` ベースの展開は別途。まずは `./keys/anthropic` のような単純相対のみ）
- `SpawnPod` の入力に子 cwd の指定を追加（cwd 明示は別チケット）
- `pod` CLI の `--pwd` 廃止後の移行期間対応（一発破壊的変更で行く）
