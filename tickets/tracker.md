# Tracker: ReadTracker のリネームと機能追加

## 背景

`tools::ReadTracker` は既に Read/Write/Edit のすべてでファイル操作を記録している
(`record()` が各ツールから呼ばれる)。名前に反して「read 以外も追跡している」状態。

また compact の改善 ([compact-improvements.md](compact-improvements.md)) で
「最近触られたファイル一覧」をデフォルトリファレンスとして compact worker に渡したい
要求があり、既存の Tracker を拡張すれば自然に解決する。

## 方針

1. `ReadTracker` → `Tracker` にリネーム (crate 全体)
2. 順序付き (recency) の履歴を追加
3. `recent_files(n)` メソッドで最近 N 件を取得できるようにする
4. Pod が Tracker を保持して compact 時に参照

## 実装

### リネーム

- `crates/tools/src/read_tracker.rs` → `crates/tools/src/tracker.rs`
- `pub struct ReadTracker` → `pub struct Tracker`
- `crates/tools/src/lib.rs` の pub 再公開 (`pub use read_tracker::ReadTracker` → `pub use tracker::Tracker`)
- crate 内呼び出し側 (`read.rs`, `write.rs`, `edit.rs`, `scoped_fs.rs` など)
- テスト (`tests/integration.rs`, `tests/edge_cases.rs`)
- `crates/pod/src/controller.rs:172` の `tools::ReadTracker::new()` → `tools::Tracker::new()`

### 内部構造の変更

```rust
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    hashes: HashMap<PathBuf, ContentHash>,
    recency: VecDeque<PathBuf>,  // 先頭が最新
}

const RECENCY_CAPACITY: usize = 20;
```

### `record()` の挙動追加

既存の hash 記録に加えて:

```rust
pub fn record(&self, path: &Path, bytes: &[u8]) {
    let key = canonicalize_or_owned(path);
    let hash = hash_bytes(bytes);
    let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
    inner.hashes.insert(key.clone(), hash);

    // LRU: 既存エントリを除去 → 先頭に push
    inner.recency.retain(|p| p != &key);
    inner.recency.push_front(key);
    if inner.recency.len() > RECENCY_CAPACITY {
        inner.recency.pop_back();
    }
}
```

### 新メソッド

```rust
/// Return up to `n` most recently recorded file paths.
/// Order: most recent first.
pub fn recent_files(&self, n: usize) -> Vec<PathBuf> {
    let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
    inner.recency.iter().take(n).cloned().collect()
}
```

### Pod への接続

- `Pod` に `tracker: Option<tools::Tracker>` フィールド追加 (builtin-tools 未登録の場合は None)
- Controller が `Tracker::new()` した時点で Pod にも `attach_tracker(tracker.clone())` で共有
- Compact 実行時 (`Pod::compact` 内) に `self.tracker.as_ref().map(|t| t.recent_files(5))` でデフォルトリファレンスを取得

### ライフサイクルの整合性

既存のドキュメント: 「Tracker は session-scoped。Controller spawn ごとに new」
この方針は維持。compact 後も同じ Controller spawn 内で状態が継続するので、
compact worker が `read_file` で追加で参照したファイルも次回 compact 時に効く。

### テスト追加

- `recent_files` が recency 順で返ること
- `RECENCY_CAPACITY` を超えた場合に古いものが落ちること
- 既存パスを再 record したら先頭に移動すること
- Read/Write/Edit 実行後に recent_files に現れること (integration テスト)

## 影響範囲

- `crates/tools/src/read_tracker.rs` — リネーム + Inner 構造体化 + recency フィールド + recent_files メソッド
- `crates/tools/src/lib.rs` — pub use 修正
- `crates/tools/src/{read,write,edit,scoped_fs}.rs` — 型名追従
- `crates/tools/tests/*` — 型名追従 + recent_files のテスト追加
- `crates/pod/src/pod.rs` — `tracker` フィールド + `attach_tracker` メソッド
- `crates/pod/src/controller.rs` — `tracker.clone()` を Pod にも渡す

## 依存

- なし (単独で実装可能)

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — デフォルトリファレンスの抽出がこれに依存
