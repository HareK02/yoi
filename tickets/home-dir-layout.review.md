# Review: ホームディレクトリ配下のディレクトリ整理

レビュー対象: HEAD (`915061f home-dirの整理`)。

## 前提・要件の確認

- **paths.rs に config/data/runtime の責務と解決ロジックが集約され、module コメントで配置が明記**: 満たされている。`crates/manifest/src/paths.rs:1-25` に三つのベースディレクトリの責務、解決順マトリクス、`INSOMNIA_HOME` 設定時の `$X/config` `$X` `$X/run` 各サブツリーへの集約が表で示されている。`config_dir` / `data_dir` / `runtime_dir` と well-known file getter (`user_manifest_path`, `user_prompts_dir`, `user_pack_file`, `user_catalog_override`, `sessions_dir`, `scope_lock_path`, `pod_runtime_dir`, `pod_socket_path`) が揃っている。
- **既存 11 箇所の置換 / `default_runtime_dir` と `default_base` の重複解消**: 満たされている。
  - `pod/src/main.rs:99-101` `paths::sessions_dir()`、`pod/src/main.rs:137-146` `paths::runtime_dir()` で旧 `default_store_dir` / `default_runtime_dir` を完全削除 (`git diff HEAD~1 HEAD -- crates/pod/src/main.rs` で確認済み)。
  - `pod/src/runtime/dir.rs:122-130` `default_base` は `paths::runtime_dir()` の薄いラッパに変わり、`main.rs` との重複が解消されている。
  - `pod/src/runtime/scope_lock.rs:69-77` `default_lock_path` は `paths::scope_lock_path()` のラッパ。
  - `pod/src/factory.rs:111-118` で `paths::user_manifest_path` / `user_prompts_dir` / `user_pack_file` を直接利用。
  - `tui/src/main.rs:32-38` `resolve_socket` は `paths::pod_socket_path()` 経由。
  - `provider/src/catalog.rs:160,193` で `paths::user_catalog_override` を利用。
  - `manifest/src/cascade.rs` から `user_manifest_path` 削除、`manifest/src/lib.rs:9` で `pub use paths::user_manifest_path` の互換 re-export。
- **`INSOMNIA_HOME` / `INSOMNIA_CONFIG_DIR` / `INSOMNIA_DATA_DIR` / `INSOMNIA_RUNTIME_DIR` の優先順位**: `paths.rs:30-68` の各関数と `paths.rs:199-292` のテストで `INSOMNIA_<KIND>_DIR > INSOMNIA_HOME > XDG_* > 既定` の順位が網羅的にカバーされている (`config_dir_explicit_wins_over_insomnia_home`, `config_dir_insomnia_home_outranks_xdg`, `runtime_dir_insomnia_home_is_run_subdir`, etc.)。
- **`INSOMNIA_SCOPE_LOCK` の廃止**: 完了。production / test 両方の grep で 0 ヒット (`tickets/home-dir-layout.md` 内の歴史的記述のみ残る)。`scope_lock.rs:545-1126` のテストは `RuntimeDirSandbox` (`INSOMNIA_RUNTIME_DIR` + `INSOMNIA_HOME` / `XDG_RUNTIME_DIR` の退避) に置換済み。
- **`unsafe std::env::set_var` を使うテスト箇所が最小限**: 妥当な水準まで縮小。`paths.rs` 12 件・`scope_lock.rs` 3 ブロック (set/remove)・`catalog.rs` 2 件・統合テスト 3 本 (各 EnvGuard + 個別 set_var) と、いずれも env override の RAII guard か serial gate を経由しており、共有ヘルパに収斂されている。元の `scope_lock.rs:980` のような scope_lock 直書きは消えた。env を弄らないと `INSOMNIA_RUNTIME_DIR` 等の優先順位を検証できないため「最小限」の解釈として妥当。
- **pod 起動 / TUI spawn flow が動作**: `cargo build --workspace` / `cargo test --workspace` 通過の旨をユーザー側で確認済みと申告 (本レビューでは再実行せず)。コードパスは旧実装と論理同型のラッパに置換されているため、機能後退の余地は小さい。

## アーキテクチャ・スコープ

- 集約先の選定 (新 crate を作らず `manifest::paths`) は memory `feedback_llm_worker_scope` / `feedback_crate_naming` の方針に整合。`manifest` crate は既に provider / pod / tui から依存されており、追加の依存グラフ歪みが発生していない。
- API 形状は **「ベース 3 つ + well-known ファイルの getter」** の薄い層に留めており、I/O やディレクトリ作成・存在検査を `paths` 側で握り込んでいない。実際の作成・検査は呼び出し側 (`scope_lock.rs:101-122` `LockFileGuard::open`, `RuntimeDir::create_default`, etc.) に残っているので、テストでの差し替えやサンドボックス運用が壊れない。
- `INSOMNIA_HOME` のレイアウトマッピング (`config = $X/config`, `data = $X` 直, `runtime = $X/run`) は、案 C (XDG_CONFIG_HOME 尊重 + `~/.insomnia/` を data/runtime) と整合。テスト用途では「単一 tempdir に三系統が同居する」直感も保たれている。
- 範囲外 (Windows / `XDG_DATA_HOME` / `XDG_STATE_HOME`) には手を出していない。YAGNI を守れている。
- `manifest/src/lib.rs:9` の `pub use paths::user_manifest_path` は、cascade.rs から関数を消した互換維持で、ext crate の API 壊しを避けている。狙いは合理的。

## 指摘事項

### Non-blocking / Follow-up

- ドキュメント未追従 (本チケットの完了条件外だが、後続のために notes): `docs/architecture.md:90,97,108,122`、`docs/pod-factory.md:16,79,183`、`docs/plan/llm_providers.md:47`、`crates/pod/src/prompt/loader.rs:8` が `$XDG_CONFIG_HOME/insomnia/...` / `$XDG_RUNTIME_DIR/insomnia/scope.lock` を直接書いており、`INSOMNIA_HOME` / `INSOMNIA_CONFIG_DIR` 系の override に触れていない。誤りではない (その経路は今も既定として有効) が、後続の `tickets/tui-user-model-setup.md` で wizard が「どこに書くか」を表示する際、ドキュメント側の表記も `manifest::paths` を起点にする方が一貫する。本チケットでは敢えて手を入れていないが、次にこの周辺を触る時にまとめて差し替えるのが自然。
- `paths::data_dir` は env が完全に何も無い場合でも `HOME` だけは要求する設計だが、`INSOMNIA_DATA_DIR` 単独設定で `HOME` 不在のケースはちゃんと拾う (`paths.rs:46-52`)。一方 `data_dir` には `XDG_DATA_HOME` のフォールスルーが無く、ticket の方針 (`$XDG_DATA_HOME` 非対応) どおりだが、将来同変数を採用する際の差分位置は `data_dir` の 2 行目になる旨をコメントしておくと意図が伝わりやすい。
- `paths::pod_socket_path` の doc コメント (`paths.rs:108-113`) は「Pod プロセスは `RuntimeDir::socket_path()` 経由で作成する」と外部からの予測 API である旨を明示しており良い。一方 `RuntimeDir::socket_path()` 側 (`crates/pod/src/runtime/dir.rs:99-101`) には逆参照が無いため、相互コメントを足すと将来の混乱予防になる (Nit に近い)。

### Nits

- `paths.rs:121-126` の `env_path` ヘルパは `std::env::var(name).ok().filter(...)` で、`Err(NotPresent)` と `Err(NotUnicode)` を共に未設定として扱う。後者は実用上ほぼ起きないが、tracing で warn を出す価値はある — ただし本チケットの責務外なので強制ではない。
- `pod/src/main.rs:100` の `unwrap_or_else(|| PathBuf::from(".insomnia/sessions"))` フォールバック (env 全滅時の相対パス退避) は、後続の `FsStore::new` で cwd 起点の相対書き出しになるため挙動が読みにくい。`runtime_dir` 側 (`main.rs:137-146`) のように early-error にするか、コメントで意図 (どうせ HOME も無い極端な構成: テスト harness のごく一部) を一行入れると良い。実害は無いので Nit。

## 判断

**Approve** — チケットの 6 つの完了条件すべてに対して具体的な根拠が確認でき、コードベースの歪みも観測されない。`paths` モジュールは責務分離・優先順位・テストカバレッジの三点で過剰でも不足でもなく、後続の `tui-user-model-setup` 等が乗ってくる土台として妥当。ドキュメント追従は本チケットの責務外として Follow-up に分類。
