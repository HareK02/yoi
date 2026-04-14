# Scope の再設計

## 背景

Scope は Pod がどのファイルにどの権限でアクセスできるかを宣言するもの。
現状の問題:

1. `Pod.scope` が `Option<Scope>` — scope なし Pod が成立しうる（ツール未登録）
2. `Scope` は `root: PathBuf` のみ — 書き込み許可/禁止の概念がない
3. 単一ディレクトリしか宣言できない — 「src は書ける、docs は読むだけ」が表現できない
4. pwd（作業ディレクトリ）と「アクセス可能領域」が同じ概念に押し込まれている
5. 将来の複数 Pod 排他制御に必要な「権限つき領域宣言」のデータ構造がない

## 方針

### pwd と scope を分離

- **pwd** は Pod の作業ディレクトリ。`[pod]` テーブルで指定。Glob/Grep のデフォルト検索パスにもなる。
- **scope** は「許可された領域」と「禁止された領域」のリスト。`[scope]` テーブルで指定。
- pwd は scope の allow に **read 以上で含まれていなければならない**。違反は manifest ロード時エラー。

### permission のレベル格子モデル

`permission` は単一文字列で能力レベルを表す:

| 値 | レベル | 意味 |
|---|---|---|
| `"read"` | 1 | 読める |
| `"write"` | 2 | 読み書きできる |

- **allow**: 「このレベル**以上**を許可」。`permission = "write"` は読み書き両方を grant。
- **deny**: 「このレベル**未満**まで引き下げる」。`permission = "write"` は write だけ落として read は残す。`permission = "read"` は read を落とすので write も連鎖して落ちる（write は read を要求するため）= 完全アクセス不可。
- **有効権限** = `min( max(allow), min(deny) )`。allow にマッチしない path は scope 外（アクセス不可）。
- 順序非依存。

### マニフェスト

```toml
[pod]
name = "agent-foo"
pwd = "./src"

[scope]

[[scope.allow]]
target = "./src"
permission = "write"
recursive = true            # 省略可、デフォルト true

[[scope.allow]]
target = "./docs"
permission = "read"

[[scope.deny]]
target = "./src/secrets.rs"
permission = "write"        # → ./src/secrets.rs は read-only に格下げ

[[scope.deny]]
target = "./src/.env"
permission = "read"         # → アクセス不可
```

ルール:
- `[scope]` は **省略不可**。なければ manifest エラー。
- `[[scope.allow]]` は **最低 1 件必須**。空 scope は無意味。
- `[[scope.deny]]` は省略可。
- `permission` は allow / deny のどちらでも必須。
- `recursive` 省略時は `true`。`false` のとき、target ディレクトリ直下のエントリのみが対象（サブディレクトリは降りない）。
- pwd は allow のいずれかで read 以上にマッチしなければならない。
- pwd が deny で read を落とされていたらエラー。

### Rust 型

```rust
// crates/manifest/src/lib.rs
pub struct PodMeta {
    pub name: String,
    pub pwd: PathBuf,
}

pub struct ScopeConfig {
    pub allow: Vec<ScopeRule>,
    pub deny: Vec<ScopeRule>,
}

pub struct ScopeRule {
    pub target: PathBuf,
    pub permission: Permission,
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read = 1,
    Write = 2,
}
```

```rust
// crates/manifest/src/scope.rs
pub struct Scope {
    allow: Vec<ScopeRule>,
    deny: Vec<ScopeRule>,
}

impl Scope {
    /// Effective permission for `path`. `None` means out-of-scope.
    pub fn permission_at(&self, path: &Path) -> Option<Permission>;

    /// Convenience: writable iff effective permission ≥ Write.
    pub fn is_writable(&self, path: &Path) -> bool;

    /// Convenience: readable iff effective permission ≥ Read.
    pub fn is_readable(&self, path: &Path) -> bool;
}
```

`permission_at` 計算:
1. allow 群のうち path にマッチするもの（recursive 考慮）から最大 permission を取る。なければ `None` を返す。
2. deny 群のうち path にマッチするものから最小 permission を取る（あれば、その値**未満**にキャップ）。
3. キャップ後のレベルが 0 なら `None`、`Read` なら `Some(Read)`、`Write` なら `Some(Write)`。

### Pod で必須化

```rust
pub struct Pod<C, St> {
    pwd: PathBuf,
    scope: Scope,    // Option ではない
    // ...
}

impl<C: LlmClient, St: Store> Pod<C, St> {
    pub async fn new(
        manifest: PodManifest,
        worker: Worker<C>,
        store: St,
        pwd: PathBuf,
        scope: Scope,
    ) -> Result<Self, PodError> { /* ... */ }

    pub async fn restore(
        session_id: SessionId,
        manifest: PodManifest,
        client: C,
        store: St,
        pwd: PathBuf,
        scope: Scope,
    ) -> Result<Self, PodError> { /* ... */ }

    pub fn pwd(&self) -> &Path { &self.pwd }
    pub fn scope(&self) -> &Scope { &self.scope }
}
```

`from_manifest` 系は manifest の `[pod].pwd` と `[scope]` から pwd / scope を構築する。整合性チェック（pwd が allow に含まれる、deny で潰されていない）はここで行う。

### ScopedFs の変更

```rust
pub fn read(&self, path: &Path) -> Result<Vec<u8>, ToolsError> {
    if !self.inner.scope.is_readable(path) {
        return Err(ToolsError::OutOfScope(path.to_path_buf()));
    }
    // ...
}

pub fn write(&self, path: &Path, content: &[u8]) -> Result<WriteOutcome, ToolsError> {
    if !self.inner.scope.is_writable(path) {
        // 読めるが書けない場合と、そもそも見えない場合を区別
        return Err(if self.inner.scope.is_readable(path) {
            ToolsError::ReadOnly(path.to_path_buf())
        } else {
            ToolsError::OutOfScope(path.to_path_buf())
        });
    }
    // ...
}
```

Glob/Grep のデフォルト検索パスは Pod の `pwd`。targets 全走査はしない。
絶対パスでの検索結果は `is_readable` でフィルタする。

### Controller 統合

```rust
// 今: scope が Some のときだけツール登録
if let Some(scope) = scope_for_tools { ... }

// 後: 常にツール登録（scope は必須）
let fs = tools::ScopedFs::new(pod.scope().clone(), pod.pwd().to_path_buf());
let tracker = tools::Tracker::new();
worker.register_tools(tools::builtin_tools(fs, tracker));
```

## 影響範囲

- `crates/manifest/src/lib.rs`
  - `PodMeta` に `pwd: PathBuf` を追加
  - `ScopeConfig` を `{ allow: Vec<ScopeRule>, deny: Vec<ScopeRule> }` に置き換え（旧 `root` フィールド削除）
  - `Permission` enum、`ScopeRule` 構造体を追加
  - `PodManifest::scope` を `Option<ScopeConfig>` → `ScopeConfig` に（省略不可）
  - manifest ロード時の整合性チェック（scope 必須、allow 1 件以上、pwd と allow/deny の関係）

- `crates/manifest/src/scope.rs`
  - `Scope` 構造体を allow/deny ベースに作り直し
  - `permission_at` / `is_readable` / `is_writable`
  - 旧 `Scope::contains` は削除（呼び出し元は `is_readable` / `is_writable` に置換）

- `crates/pod/src/pod.rs`
  - `scope: Option<Scope>` → `scope: Scope`
  - `pwd: PathBuf` フィールド追加
  - `Pod::new` / `Pod::restore` シグネチャ変更
  - `Pod::scope()` の戻り値を `Option<&Scope>` → `&Scope` に
  - `from_manifest` 系は manifest から pwd / scope を取り出して新シグネチャに渡す

- `crates/tools/src/scoped_fs.rs`
  - 内部の scope 参照を新 API（`is_readable` / `is_writable`）に置換
  - `pwd` を別途保持（Glob/Grep のデフォルト検索パス用）
  - 既存の境界チェックを置換

- `crates/tools/src/error.rs`
  - `ReadOnly(PathBuf)` variant を追加
  - 既存の `OutOfScope` は維持

- `crates/pod/src/controller.rs`
  - `scope` の Optional 分岐を削除し、常時ツール登録

- `crates/pod/src/prune.rs` 等の scope 参照箇所
  - `Option<&Scope>` 前提の処理を `&Scope` 前提に書き換え

- 既存テスト
  - `manifest` のパースケース更新（新 TOML 構文）
  - `tools::scoped_fs` のテスト更新（権限レベル別、deny 部分化のケース追加）

## 確認済み論点

- **target の解決基準**: `[scope]` 内の `target` は manifest の `[pod].pwd` からの相対パスとして解決する。manifest ロード時に pwd と合わせて絶対パスへ正規化。
- **deny 合成**: マッチする deny が複数あれば min(permission) を取り、その値未満にキャップ。順序非依存・冪等。
- **Glob/Grep の検索起点**: 既存の `path` パラメータを保持し、省略時のデフォルトを旧 `scope.root()` から **pwd** に切り替えるだけ。pwd 外に絶対パスで検索された場合、結果は `is_readable` でフィルタする。越境検索禁止の特別ロジックは入れない。

## 非ゴール

- **複数 Pod 間の scope 排他制御** は本 ticket では扱わない。別 ticket [scope-exclusion.md] に切り出す。本 ticket では「将来 read 共有 / write 排他の単位として `Scope` を使えるようにデータ構造を整える」までで止める。
- **bash ツールへの execute 権限** は本 ticket では扱わない。bash-tool ticket / permission-extension-point ticket で扱う。`Permission` は当面 `Read` / `Write` の 2 値。
