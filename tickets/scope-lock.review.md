# レビュー: Scope lock file

対象差分: `crates/pod/src/scope_lock.rs` (新規 992行), `crates/manifest/src/scope.rs`, `crates/pod/src/{pod,lib,runtime_dir}.rs`, `crates/pod/Cargo.toml`（未コミット）

## 要件達成状況

| 要件 | 状態 |
|---|---|
| lock file に Pod の scope allocation を記録 | ✅ `register_pod` / `delegate_scope` で JSON に書き込み |
| flock による排他アクセス | ✅ `LockFileGuard::open` で `lock_exclusive`、Drop で `unlock` |
| write 衝突の検出 | ✅ `find_conflict_owner` が delegation tree を下降して真の所有者を特定 |
| stale エントリの自動回収 (PID 死活) | ✅ `reclaim_stale` が `kill(pid, 0)` で判定、dead を削除・子を reparent |
| scope 分譲の記録 (`delegated_from`) | ✅ `delegate_scope` が spawner の effective scope 内か検証してから登録 |
| 分譲チェーンの reparent (A→B→D、B 死亡時 D を A に付け替え) | ✅ `release_pod` / `reclaim_stale` ともに `delegated_from` を親に付け替え |
| Pod 正常終了時の allocation 解放 | ✅ `ScopeAllocationGuard` の Drop で `release_pod` を呼ぶ |
| Pod 起動時 (`from_manifest`) に自動登録 | ✅ `scope_lock::install_top_level` を `from_manifest` 内で呼出 |
| テストで衝突検出・stale 回収・分譲/返却を検証 | ✅ 16 テストケース |
| パーミッション制御 (0600) | — チケットに記載あるが、ファイル作成時の umask 制御は未実装。`OpenOptions` に mode 設定なし |

## アーキテクチャ

### 良い点

**`LockFileGuard` の RAII 設計**: `open` で flock 取得、`save` で書き込み、Drop で unlock。mutate-but-don't-save のパスでは変更が破棄される（エラー時に安全）。

**`ScopeAllocationGuard` で Pod ライフサイクルに紐付け**: Pod 構造体が `Option<ScopeAllocationGuard>` を保持し、Drop で lock file から自動削除。`Pod::new` / `Pod::restore`（テスト用）は `None` でバイパス。

**`find_conflict_owner` が delegation tree を下降**: 衝突エラーに「真の所有者」（最深のノード）を表示。`conflict_detection_descends_to_real_owner` テストで lock-in。

**`reclaim_stale_with` のテストシーム**: PID 生存判定を引数で差し替え可能。テストで任意の PID を「dead」にできる。

**`is_within_effective_write`**: spawner の allow set から子に委譲済みの部分を差し引いた「実効 scope」を計算。`delegate_scope` のバリデーションに使用。

**`rules_overlap` の 4 パターン分岐** (recursive×recursive, recursive×non, non×recursive, non×non): 非再帰ルールの覆域（target 自身 + 直下の子）を正しく考慮。

### 指摘事項

#### 1. 🟡 ファイルパーミッションの明示設定

チケットの「セキュリティとアクセス」セクション:
> ファイルパーミッション `0600`（owner only）、ディレクトリは `0700`

`LockFileGuard::open` は `OpenOptions::new().create(true)` で作成しているが、パーミッションの明示設定がない。umask がデフォルト (0022) なら `0644` になり、他ユーザーから読めてしまう。

```rust
// 現状
let file = OpenOptions::new()
    .read(true).write(true).create(true).truncate(false)
    .open(path)?;

// 修正案: Unix 拡張で mode を設定
use std::os::unix::fs::OpenOptionsExt;
let file = OpenOptions::new()
    .read(true).write(true).create(true).truncate(false)
    .mode(0o600)
    .open(path)?;
```

ディレクトリも `create_dir_all` の後に `std::fs::set_permissions` で `0700` に制限する。

**判断**: セキュリティ要件。修正すべき。

#### 2. 🟢 `socket` フィールドの予測パス

`pod.rs` で socket path を `runtime_dir::default_base()?.join(&manifest.pod.name).join("sock")` と予測しているが、実際の `RuntimeDir` が作る socket path と一致するか保証がない（`RuntimeDir` のパス構築ロジックが変わったら乖離する）。

現時点では一致しているが、将来 `RuntimeDir` のパス規則が変わったとき scope_lock の socket 情報が嘘になる。`RuntimeDir` 側に `fn socket_path_for(pod_name: &str) -> PathBuf` のような static メソッドを置いて、両者が同じ関数を呼ぶようにするとより堅牢。

**判断**: 現時点では問題なし。リファクタ余地として認識。

#### 3. 🟢 `covers_fully` の permission 比較

```rust
fn covers_fully(cover: &ScopeRule, inner: &ScopeRule) -> bool {
    if cover.permission < inner.permission {
        return false;
    }
    ...
}
```

`Permission` の `Ord` は `Read < Write`。`cover.permission < inner.permission` = 「cover が Read で inner が Write なら不十分」。正しい。

#### 4. 🟢 テストの網羅性

16 ケース:
- ファイル操作: open creates / save-reopen roundtrip
- overlap 判定: prefix relation / non-recursive
- 登録: write conflict / duplicate name / read doesn't conflict
- 分譲: must be subset / succeeds within parent / rejects sibling overlap
- 解放: reparents children / reopens scope / returns to parent
- stale: reparents and removes dead entries
- guard: releases on drop
- conflict detection: descends to real owner

delegation tree の主要シナリオ (A→B→D) がカバーされている。

## 結論

**指摘1 (ファイルパーミッション 0600) の修正を条件に受け入れ可**。他は問題なし。実装の核心（delegation tree walk, effective scope 計算, stale reclaim + reparent）が正確で、テストが充実している。
