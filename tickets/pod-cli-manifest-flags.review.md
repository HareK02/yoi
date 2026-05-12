# Review: Pod CLI: マニフェスト関連フラグの整理

## 前提・要件の確認
- `--user-manifest <PATH>` の削除: 満たされている。`Cli` から `user_manifest` フィールドは消え、代わりに `manifest` / `project` / `overlay` が定義されている（`crates/pod/src/main.rs:17-32`）。`--user-manifest` が clap 上 unknown argument になるテストも追加されている（`crates/pod/src/main.rs:279-283`）。
- `INSOMNIA_USER_MANIFEST` による user manifest override: 満たされている。`resolve_manifest` が `std::env::var_os(USER_MANIFEST_ENV)` を読み、非空の場合のみ `PathBuf` に変換して `with_user_manifest` 経路へ渡している（`crates/pod/src/main.rs:59-91`, `crates/pod/src/main.rs:102-115`）。明示 path は既存の `with_user_manifest` により missing file が error になる（`crates/pod/src/factory.rs:123-131`）。override のテストもある（`crates/pod/src/main.rs:332-349`）。
- env 空文字列は未指定扱い: 満たされている。`OsString::is_empty()` の場合 `None` に落としている（`crates/pod/src/main.rs:84-91`）。`--manifest` との併用でも空 env は許可されるテストがある（`crates/pod/src/main.rs:317-330`）。
- `--manifest <PATH>` の単一ファイル mode: 満たされている。`--manifest` 指定時は factory/cascade 経路へ進まず `load_single_manifest` に return し、`PodManifest::from_toml` と `PromptLoader::builtins_only()` で構築している（`crates/pod/src/main.rs:69-75`, `crates/pod/src/main.rs:94-99`）。
- `--manifest` と `--project` / `--overlay` の併用不可: 満たされている。clap の `conflicts_with_all = ["project", "overlay"]` が付いており（`crates/pod/src/main.rs:18-21`）、conflict テストもある（`crates/pod/src/main.rs:285-301`）。
- `--manifest` と `INSOMNIA_USER_MANIFEST` の併用不可: 満たされている。非空 env がある場合は `load_single_manifest` 前に error を返す（`crates/pod/src/main.rs:67-75`）。テストもある（`crates/pod/src/main.rs:303-315`）。
- 既存の `--project` / `--overlay` / `--adopt` / `--callback` 挙動保持: 満たされている。cascade mode 側では従来通り project 探索と overlay 適用を行い（`crates/pod/src/main.rs:117-130`）、Pod 生成側の adopt/callback/session 分岐は flag 整理の外側で維持されている（`crates/pod/src/main.rs:173-204`）。
- `pod --overlay <toml>` 起動の互換: 満たされている。CLI 上 `--overlay` は残り、`build_factory_with_user_manifest_path` で従来通り `with_overlay_toml` に渡される（`crates/pod/src/main.rs:29-32`, `crates/pod/src/main.rs:126-130`）。手元の `start_pod.local.fish` は `--overlay` のみを使っており、削除された `--user-manifest` には依存していない（`<repo>/start_pod.local.fish:5`）。

## アーキテクチャ・スコープ
- 変更は `crates/pod/src/main.rs` の CLI entrypoint に閉じており、manifest cascade 本体や `PodFactory` の責務を拡張していない。`--manifest` は ticket 指定通り `PodManifest::from_toml` の直接経路で、cascade mode は既存 factory を使い続けるため、層境界は保たれている。
- 単一ファイル mode の prompt loader は `PromptLoader::builtins_only()` として最小構成で組まれており、user / workspace prompt layer や pack file の自動探索を持ち込んでいない（`crates/pod/src/main.rs:94-99`）。
- 追加依存や crate 構成変更はなく、ticket scope に対して過剰な実装は見当たらない。

## 指摘事項
### Non-blocking / Follow-up
- `docs/pod-factory.md` の CLI 説明が旧仕様のまま残っている — `--user-manifest` を掲載し、`--manifest` と `INSOMNIA_USER_MANIFEST` を説明していない（`docs/pod-factory.md:300-312`）。ticket の完了条件ではないため blocking にはしないが、利用者向けドキュメントとして追随更新した方がよい。

## 判断
Approve with follow-up — ticket の完了条件は満たされており blocking はないが、既存ドキュメントに旧 CLI フラグが残っているため follow-up として扱う。
