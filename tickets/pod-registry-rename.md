# `scope-lock` → `pod-registry` リネーム

## 背景

`crates/scope-lock` は元々 `crates/pod/src/runtime/scope_lock.rs` にあった機能を、TUI picker から live セッション判定 (`lookup_session`) のために参照したいという理由で独立クレートに切り出したもの。実体は「マシン全体で生きている Pod の allocation テーブル」であり、scope 衝突判定はその一機能にすぎない（session_id 衝突防止・delegation tree の reparent・stale 回収などは scope と独立に効いている）。

加えて、永続化先のファイル名 `scope.lock` も実態と合っていない:

- 中身は `flock(2)` で保護された JSON ステート。`Cargo.lock` のようなバージョン固定ファイルではない
- `flock` は read-modify-write のトランザクションを直列化するためだけに保持しており、Pod の生存期間ずっとロックを保持しているわけでもない
- 拡張子で `.lock` を名乗っていることで「触るな」という誤った印象を与える

## ゴール

クレート名とファイル名を実態に合わせる:

- crate `scope-lock` → `pod-registry`
- ファイル `<runtime_dir>/scope.lock` → `<runtime_dir>/pods.json`
- `manifest::paths::scope_lock_path` → `manifest::paths::pod_registry_path`
- インポート `scope_lock::...` / `crate::runtime::scope_lock::...` → `pod_registry::...`
- `crates/pod/src/runtime/mod.rs` の `pub use ::scope_lock` → `pub use ::pod_registry`
- ドキュメンテーション・コメント中の "scope.lock" / "scope-lock registry" の言い換え

## 範囲外

- 内部型名のリネーム（`LockFile` / `LockFileGuard` / `ScopeAllocationGuard` / `ScopeLockError` 等）。これらは内部 API でリネームしても挙動は変わらず、必要が生じた時に別途整理する。
- `ScopeRule` / `scope_allow` / `delegate_scope` 等、scope そのものを扱うシンボル名は据え置き（クレートが何かではなく、何を扱うかなので）。
- 既存の `scope.lock` ファイルが残っている環境での互換性（dev 期間中の互換不要、必要なら手で消す）。

## 完了条件

- `crates/pod-registry` として独立クレートが存在し、`pod`/`tui` の依存もこの名前を指している
- `<runtime_dir>/pods.json` が新しいレジストリの保存先として使われる
- 既存テスト（pod / pod-registry / tui 関連）がすべて緑
- ドキュメンテーション・チケット本文中の参照（`tickets/dynamic-scope.md` 等）が新名に揃っている

## Review
- 状態: Approve
- レビュー詳細: [./pod-registry-rename.review.md](./pod-registry-rename.review.md)
- 日付: 2026-04-29
