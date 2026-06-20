---
title: 'Remove obsolete daemon crate'
state: 'closed'
created_at: '2026-06-20T13:36:13Z'
updated_at: '2026-06-20T13:41:19Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-06-20T13:36:51Z'
---

## 背景

`crates/daemon/` は long-lived Pod lifecycle management 用に予約されていた placeholder crate だが、実装責務を持たないまま放置されている。現在の Pod lifecycle / socket serving / CLI/TUI startup は他 crate が所有しており、daemon crate を workspace に残すことで Cargo workspace、Cargo.lock、検証対象、設計境界に不要なノイズが残っている。

`daemon` という名前の将来責務は、Plugin Service/Ingress や Pod lifecycle の設計が固まってから、具体的な authority boundary と work item に基づいて再導入する。

## 要件

- `crates/daemon/` を削除する。
- root `Cargo.toml` の workspace `members` / `default-members` から `crates/daemon` を削除する。
- `Cargo.lock` から空の `daemon` package entry が消えるように更新する。
- placeholder crate に依存する build/test/package path が残っていないことを確認する。
- TUI completion tests など、実体パスではなく fixture として `crates/daemon` を参照している箇所は、別の現存 crate 名に置き換える。
- historical report の過去記録は必要がない限り追跡対象から削除しないが、active docs / build config は obsolete crate を前提にしない。

## 受け入れ条件

- `crates/daemon/` が repository から削除されている。
- `cargo metadata` / `cargo check` が daemon workspace member 不在で成功する。
- `cargo test -p tui` など daemon 名を fixture にしていたテストが通る。
- repository の active build/config/code references に `crates/daemon` が残っていない。
- Validation before completion includes `cargo fmt --check`, relevant `cargo test`/`cargo check`, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`.
