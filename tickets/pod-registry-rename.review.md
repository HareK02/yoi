# Review: `scope-lock` → `pod-registry` リネーム

## 前提・要件の確認

- crate `scope-lock` → `pod-registry`: `crates/pod-registry/Cargo.toml:2` で `name = "pod-registry"` に変更済み。`Cargo.toml:12` のワークスペース members、`crates/pod/Cargo.toml:15`、`crates/tui/Cargo.toml:18` も `pod-registry` を参照する。古い `crates/scope-lock` ディレクトリは削除されている。
- ファイル `<runtime_dir>/scope.lock` → `<runtime_dir>/pods.json`: `crates/manifest/src/paths.rs:99-101` の `pod_registry_path()` が `runtime_dir().join("pods.json")` を返し、`crates/pod-registry/src/table.rs:71-79` の `default_registry_path()` がこの API を経由する。テスト中の path 構築 (`mutate.rs:200`、他) も全て `pods.json` を直接書いている。
- `manifest::paths::scope_lock_path` → `pod_registry_path`: 旧 API のシンボルは消え、新 API のみ存在する (`paths.rs:99` ; `paths.rs:305-307` のテストも更新済み)。
- import の差し替え: `crates/pod/src/pod.rs:29`、`crates/pod/src/spawn/tool.rs:29`、`crates/pod/src/spawn/comm_tools.rs`、`crates/pod/src/ipc/event.rs:30`、`crates/tui/src/picker.rs:22`、テストファイル群 (`crates/pod/tests/{pod_comm_tools_test.rs,spawn_pod_test.rs,pod_events_test.rs,restore_test.rs}`) すべてで `pod_registry::...` または `crate::runtime::pod_registry::...` を使用。古い `scope_lock` は `grep` で 0 ヒット (本ファイル群 + tickets を除く)。
- `pub use ::scope_lock` → `pub use ::pod_registry`: `crates/pod/src/runtime/mod.rs:2` で更新済み。
- ドキュメンテーション・コメント: `crates/protocol/src/lib.rs:42`、`crates/manifest/src/scope.rs:145-147`、`crates/manifest/src/paths.rs:9,55`、`crates/pod/src/spawn/tool.rs:3-7,96`、`crates/pod/src/ipc/event.rs:11`、`crates/pod-registry/src/lib.rs:1-14` 等、新名で揃っている。
- `tickets/dynamic-scope.md`: 旧記述「scope lock file」を「pod-registry」に置換し、`tickets/scope-lock.md` への dead link を含む `## 依存` セクションを削除。リネーム後の整合として妥当。
- `cargo build --workspace`: 緑 (warnings はリネーム無関係の `llm-worker` の dead_code 1 件のみ)。
- `cargo test -p pod-registry`: 24 tests 全部 pass。
- `cargo test --workspace`: 全テスト pass、新たな失敗なし。

## アーキテクチャ・スコープ

- 範囲外宣言の遵守: 内部型名 (`LockFile` / `LockFileGuard` / `ScopeAllocationGuard` / `ScopeLockError`) は据え置き (`error.rs:11`、`lifecycle.rs:18`、`table.rs:15,89`)。`ScopeRule` / `scope_allow` / `delegate_scope` 等の scope 用語も変更なし。チケット明示の方針通り。
- crate 命名: `pod-registry` はメモリ「新規クレートに `insomnia-` プレフィックス不要、短い名前で統一」に沿う。
- 依存導入: `Cargo.toml` の dependency 構成はリネーム前の `scope-lock` から移動しただけで、新規依存追加は無し。`cargo add` フロー違反なし。
- 公開境界: 5 モジュール分割後も `lib.rs` の `pub use` 集合は分割前と同じ surface (`Allocation`, `LockFile`, `LockFileGuard`, `default_registry_path`, `register_pod`, `delegate_scope`, `release_pod`, `reclaim_stale`, `reclaim_stale_with`, `install_top_level`, `adopt_allocation`, `update_session`, `lookup_session`, `SessionLockInfo`, `ScopeAllocationGuard`, `ScopeLockError`, `find_conflict_owner`, `is_within_effective_write`)。`rules_overlap` / `covers_fully` は crate-private のまま、無闇に widen されていない。
- モジュール分割の責務分離: `error.rs` (型) / `table.rs` (on-disk + guard) / `conflict.rs` (read-only 判定) / `mutate.rs` (`LockFileGuard` を取る変更操作) / `lifecycle.rs` (entry point + Drop ガード) / `test_util.rs` (#[cfg(test)] 共有) は意味的に直交しており、レイヤ依存も `lifecycle → mutate → conflict → table` / `error` の DAG で循環なし。
- `mutate.rs` のサイズ (567 行) は実装 ~190 行・テスト ~377 行であり、肥大化しているのはテスト群。テストは register / delegate / release / reclaim / session_id 衝突 / sibling overlap など同一「変更系操作」のセットを集約していて、責務は揃っている。さらに細分化する必然性は薄い。
- `test_util.rs` の抽出: `RuntimeDirSandbox` は env を弄るので `ENV_LOCK` を介して serialise する必要があり、`sid` / `sock` / `write_rule` / `read_rule` / `open_empty` 含めて 4 つのテストモジュール (table / mutate / conflict / lifecycle) で重複していたため、共通化は妥当。`pub(crate)` 限定で外には漏れない。

## 指摘事項

### Non-blocking / Follow-up

- `crates/pod/src/spawn/tool.rs:171,177` の `ToolError` メッセージが "scope lock path" / "scope lock open" のまま。型名 (`LockFileGuard`) は据え置きだが、これは LLM に届く文字列なので "pod registry path" / "pod registry open" のほうが新名と整合する。範囲外宣言の「内部型名のリネームは見送り」とは別軸 (ユーザー向け文言) なので、好みで合わせるとよい。
- `crates/pod/tests/pod_events_test.rs:388` のコメント "Allocation is gone from the scope lock." も同種の言い換え候補。
- `crates/pod/src/spawn/comm_tools.rs:282`、`crates/pod-registry/src/lifecycle.rs:14,15,41,144`、`crates/pod-registry/src/table.rs:81,95`、`crates/pod-registry/src/mutate.rs:2`、`crates/pod/src/pod.rs:102` 等の "lock file" 文言は、現存する型名 `LockFile` / `LockFileGuard` を指しているため意味的に正しい。型名リネームを将来やるならこれらの文言も追随、というだけ。
- `tickets/dynamic-scope.md` の更新: dead link 削除と語彙置換は妥当。チケット本文の「pod-registry は登録・削除・衝突チェックを任意のタイミングで行えるためレジストリ側の制約はない」は元の「lock file 側の制約はない」をリネームに合わせて素直に置き換えており、意味の改変なし。

### Nits

- `crates/protocol/src/lib.rs:42` のコメント `(registry / pod-registry updates)` は「registry」と「pod-registry」が並ぶと文脈がやや冗長 (「registry」は `SpawnedPodRegistry` / `pods.json` のどちらを指すかが不明瞭)。今ある記述で誤解は実害ない範囲。

## 判断

Approve — チケットの前提・要件 (クレート名 / ファイル名 / API 名 / インポート / `pub use` / doc 参照 / dead-link 整理) はすべて達成され、範囲外宣言された内部型名は据え置かれている。ビルドと全テストが緑。モジュール分割は既存 public surface を広げず、責務の DAG も健全で、コードベースを歪めていない。残るのは `ToolError` 文言の "scope lock" 等の小さな言い換え候補のみで、blocking ではない。
