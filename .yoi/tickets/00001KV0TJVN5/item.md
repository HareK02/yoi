---
title: 'E2E harness が最新 yoi binary を自動 build して使うようにする'
state: 'inprogress'
created_at: '2026-06-13T15:46:07Z'
updated_at: '2026-06-13T15:54:18Z'
assignee: null
readiness: 'ready'
queued_by: 'yoi ticket'
queued_at: '2026-06-13T15:46:29Z'
---

## 背景

`00001KSKBP9YG` の E2E harness first slice では `YOI_E2E_BIN` または推測された `target/debug/yoi` を process-under-test として使っていた。これだと任意タイミングの `cargo test -p yoi-e2e --features e2e ...` 実行時に、最新 source から build された `yoi` binary が使われる保証がない。

ユーザー判断:
- `cargo run` を process-under-test にするより、E2E harness が test setup で `cargo build -p yoi --features e2e-test --bin yoi` を実行し、生成された binary を直接 PTY spawn する方針で修正する。

## 要件

- `YOI_E2E_BIN` が明示されていない通常 E2E 実行では、harness が workspace root で `cargo build -p yoi --features e2e-test --bin yoi` を実行してから、生成された binary path を使う。
- `cargo run` を PTY の process-under-test にしない。PTY / Ctrl+C / Quit latency 測定対象は `yoi` binary 本体にする。
- `YOI_E2E_BIN` は明示 override として残してよい。
- 複数 test で build が重複しすぎないよう、可能なら `OnceLock` 等で同一 test process 内 1 回に寄せる。
- artifact / error message に binary provider / build command / binary path が分かる情報を残す。
- 既存の production/non-production boundary、`e2e-test` feature gating、mouse capture tracking、quit pending barrier を壊さない。

## 受け入れ条件

- `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` だけで、事前の手動 `cargo build -p yoi --features e2e-test` なしに E2E が実行できる。
- E2E 実行時に build された `target/debug/yoi` が PTY に直接 spawn される。
- `YOI_E2E_BIN=<path>` 指定時は override としてその path が使われる。
- 既存 Panel E2E 2 本が pass する。
- `cargo fmt --check`、`git diff --check`、関連 package check が pass する。
