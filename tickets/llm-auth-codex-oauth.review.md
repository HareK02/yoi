# Codex OAuth 認証の流用 — レビュー

判定: **Approved**（ブロッカーなし）

## 指摘事項

### 1. `ensure_fresh` の double-load が冗長（軽微）

`crates/provider/src/codex_oauth/mod.rs` 104 行目と 114 行目で、mutex 保持下に `auth_json::load()` を 2 回呼んでいる。1 回目で stale 判定、2 回目を guarded reload として別プロセス差分検知に使う意図だが、mutex 取得直後に連続 load しているので間隔がほぼゼロで実効的な差分検知になっていない。`persist_refreshed` 内の merge に任せ、`pre_refresh` を使わず `snap` をそのまま refresh 材料にするほうが素直。

判断: 他プロセスとの競合検知を「保険として二段階で絞る」意図なら害はない。このままでも OK。

### 2. `pub use PermanentReason` を `pub(crate)` に絞る（軽微）

`crates/provider/src/codex_oauth/mod.rs` 36 行目。クレート外利用者がいないので `pub(crate) use` で十分。

判断: 気が向いたら直す。

### 3. AGENTS.md の git 操作指示の追記

本チケット実装と独立した変更。commit 時は別コミットに切り出すか、本コミットで「ついで」と明記する。

判断: commit 作成時の注意事項。実装自体への指摘ではない。

### 4. `Arc::new(CodexAuthProvider)` の二重 Arc（軽微）

`crates/provider/src/lib.rs` で `ResolvedAuth::Custom(Arc::new(provider))` としており、`CodexAuthProvider` 自体も内部に `Arc<Mutex<State>>` を持つため Arc が二段。`CodexAuthProvider: Clone` にして単段で扱える余地はある。

判断: MVP として OK。

## 結論

4 件とも軽微な改善候補で、いずれも機能・正しさ・設計方針に影響しない。チケット要件はすべて満たされており、テストも通っている。完了として問題なし。
