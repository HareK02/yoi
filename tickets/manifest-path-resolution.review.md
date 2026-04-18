# Review: manifest-path-resolution

実装コミット `aed46e6`（マニフェスト解決の相対パス化）に対するレビュー。`cargo build` clean、`cargo test --workspace` 全 pass。

## 総評

チケット要件（`pod.pwd` 削除、相対パス = manifest ファイル基準、overlay は cwd 基準、マージ前絶対化）を忠実に実装。テストもカスケード各層・不変条件違反・両 manifest レイヤの相対解決まで網羅。ビルド・テスト通過。

指摘 1 の UX 判断だけ合意したら完了可。

## 完了条件の対応

| 要件 | 状態 | 確認箇所 |
|---|---|---|
| `pod.pwd` 削除 | ✅ | `PodMeta` と `PodMetaConfig` から削除 |
| Pod pwd = cwd | ✅ | `pod.rs:current_pwd()` = `current_dir().canonicalize()` |
| 相対 = manifest ファイル基準 | ✅ | `PodManifestConfig::resolve_paths(base)` + `factory::manifest_base()` |
| overlay は cwd 基準 | ✅ | `factory::resolve_and_merge_overlay` で `current_dir()` を base |
| マージ前に絶対化 | ✅ | `factory::resolve()` で各層 `.resolve_paths(&base)` → merge |
| `ensure_absolute` は不変条件チェックのみ | ✅ | `TryFrom` に残存、cascade 層で通れば通るだけ |
| `--pwd` 廃止 | ✅ | CLI から削除 |
| `api_key_file = "keys/anthropic"` 動作 | ✅ | `resolve_paths_joins_relative_api_key_file` テスト |
| `scope.allow target = "."` 動作 | ✅ | `project_manifest_relative_paths_resolve_against_insomnia_dir` テスト |

## 指摘と判断

### 1. project manifest の base が `.insomnia/` であることの UX（要判断）

**状況**: 実装は `<project>/.insomnia/manifest.toml` に `target = "."` と書いた場合、**`<project>/.insomnia/`** を指す。チケットの「manifest ファイルのあるディレクトリ基準」を忠実に実装した結果。

```rust
// factory.rs:538 テスト
// project manifest 内 target = "." が .insomnia/ ディレクトリに解決される
assert_eq!(manifest.scope.allow[0].target, insomnia_dir);
```

**懸念**: ユーザが「project manifest で `target = "."`」と書いたら自然には「プロジェクトルート」を意図する。`.insomnia/` 下を scope の対象にしたい運用は稀。

**選択肢**:
- **(a)** このまま。規則の一貫性（全層 "manifest と同じ dir"）を優先し、ドキュメントで「`target = ".."` でプロジェクトルート」と案内
- **(b)** project manifest のみ base を親 `<project>/` にする。user manifest と挙動が揃わなくなる例外ルール
- **(c)** `.` 表記を使わず「絶対パス or `..` 表記」を案内のみ

**判断**: **(a) を推奨**。user manifest (`$XDG_CONFIG_HOME/insomnia/`) で `keys/anthropic` が `$XDG_CONFIG_HOME/insomnia/keys/anthropic` を指すのと同じ規則で、シンプル。ただし project で違和感が強いので README / docs で**典型例のテンプレ**を提示する必要あり（例: `target = ".."` で project root）。

この判断を合意できれば完了条件を満たす。(b) を選ぶなら `factory.rs:manifest_base` から派生した project 専用の親ディレクトリ計算に差し替える小改修が必要（1 関数程度）。

### 2. `pod.pwd` を書いた古い manifest の警告が埋もれる

**状況**: `from_toml_accepts_unknown_field` テストで確認されている通り、`pod.pwd = "/obsolete"` は `serde_ignored` の warn でスキップされる。`tracing_subscriber` が WARN 有効になっていないと出ない。

**判断**: 範囲外だが、移行ユーザが「設定したのに効かない」と混乱するリスクあり。README / CHANGELOG にマイ採用しないション注記を入れたい（本チケットに含めるか、別 issue を切るかは任意）。本チケットの完了条件には影響しない。

### 3. 軽微

- `PodManifestConfig::resolve_paths` の `debug_assert!(base.is_absolute())` は release で落ちないが、現状の呼び出し側（`manifest_base` / `current_dir`）が絶対を保証するので許容
- `spawn_pod.rs` で overlay TOML から `pod.pwd` を消し、`Command::current_dir(spawner_pwd)` で子に cwd を伝える構造へ正しく移行
- `Scope::from_config` の signature 変更（`base` 引数削除）が全呼出箇所に反映されている
- 不変条件違反（`ResolveError::RelativePath` / `ScopeError::RelativeTarget`）が両層でチェックされていて、万一 cascade 解決漏れがあっても catch される二重防衛になっている

## 完了に向けた作業

- 指摘 1 について (a) で合意 → このままマージ
- ドキュメントでの「`target = ".."` で project root」ガイドは別タスク化可（本チケット範囲外扱い）
