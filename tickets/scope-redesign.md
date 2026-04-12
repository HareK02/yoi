# Scope の再設計

## 背景

Scope は Pod の作業ディレクトリとアクセス権限を定義するもの。全ての Pod が必ず持つ。

現状の問題:
1. `Pod.scope` が `Option<Scope>` — scope なしの Pod が存在しうる（ツールが登録されない）
2. `Scope` が `root: PathBuf` しか持たない — 書き込み許可/禁止の概念がない
3. Scope なしの場合、Glob/Grep のデフォルト検索パスがない
4. マニフェストで `[scope]` 省略時のフォールバックが定義されていない

## 方針

### Scope の拡張

```rust
pub struct Scope {
    pwd: PathBuf,      // 作業ディレクトリ
    writable: bool,    // false = 読み取り専用 Pod
}
```

- `writable: true`（デフォルト）: Read/Write/Edit/Glob/Grep 全て使える
- `writable: false`: Read/Glob/Grep のみ。Write/Edit はエラー
- `pwd` は Glob/Grep のデフォルト検索パスとしても使われる

### Pod で必須化

```rust
pub struct Pod<C, St> {
    scope: Scope,  // Option ではない
    // ...
}
```

### マニフェスト

```toml
# 明示指定
[scope]
pwd = "./src"
writable = false   # 省略時 true

# [scope] 省略時 → マニフェストファイルの親ディレクトリが pwd、writable = true
```

`Pod::new()`（マニフェストなし構築）では Scope を引数で必須にする。

### ScopedFs の変更

```rust
pub fn write(&self, path: &Path, content: &[u8]) -> Result<WriteOutcome, ToolsError> {
    if !self.inner.scope.writable() {
        return Err(ToolsError::ReadOnly);
    }
    if !self.inner.scope.contains(path) {
        return Err(ToolsError::OutOfScope(path.to_path_buf()));
    }
    // ...
}
```

### Controller 統合

```rust
// 今: scope が Some のときだけツール登録
if let Some(scope) = scope_for_tools { ... }

// 後: 常にツール登録（scope は必須）
let fs = tools::ScopedFs::new(pod.scope().clone());
let tracker = tools::Tracker::new();
worker.register_tools(tools::builtin_tools(fs, tracker));
```

## 影響範囲

- `manifest::Scope` — `root` → `pwd`、`writable` フィールド追加
- `manifest::ScopeConfig` — `root` → `pwd`、`writable` の serde 対応
- `manifest::PodManifest` — [scope] 省略時のフォールバック解決
- `Pod` — `scope: Option<Scope>` → `scope: Scope`
- `Pod::new()`, `Pod::restore()`, `Pod::from_manifest()` — シグネチャ変更
- `ScopedFs` — writable チェック追加
- `Controller` — scope の Optional 分岐を削除
- `tools::error::ToolsError` — `ReadOnly` variant 追加
- `Scope::contains` — `root` → `pwd` のリネーム
