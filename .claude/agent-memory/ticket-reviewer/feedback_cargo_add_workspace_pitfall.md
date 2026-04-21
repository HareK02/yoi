---
name: cargo add workspace pitfall
description: ルート Cargo.toml に [workspace.dependencies] が未定義なので workspace = true は使えない
type: feedback
---

ルート `Cargo.toml` は `[workspace.package]` のみを持ち `[workspace.dependencies]` を
定義していない。したがってチケットや PR に
`foo = { workspace = true, features = [...] }` と書かれていても、そのままでは解決しない。

**Why:** プロジェクトの現状流儀として、各クレートは直接バージョン指定する
(例: `crates/session-store/Cargo.toml` の `uuid = { version = "1", features = [...] }`)。
protocol-design (2026-04-21) レビュー時に発見。

**How to apply:** チケットに `workspace = true` の文言を見たら、
- 実装が直接バージョン指定にしていれば「コードベース流儀に整合」として Follow-up 扱い、
- `workspace = true` のまま書かれていたら「ビルドが通らないはず」として Request changes、
- もしくは `[workspace.dependencies]` を整備する方向の提案を添える。
