---
title: 'Workspace server binary and CLI launcher'
state: 'inprogress'
created_at: '2026-06-21T10:15:30Z'
updated_at: '2026-06-21T10:16:04Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-06-21T10:16:04Z'
---

## 背景

Workspace web control plane bootstrap により `crates/workspace-server` は library crate として HTTP router / `serve(...)` / SQLite store を提供している。一方で、現状は product CLI から直接起動できず、手元で試すには別の harness が必要になる。

方針として、workspace server は `yoi` binary にリンクして内包しない。`crates/workspace-server` 側に独立 binary entrypoint を置き、`yoi workspace serve` は外部 `yoi-workspace-server` executable を解決して exec/spawn する薄い launcher にする。

## 要件

- `crates/workspace-server/src/main.rs` を追加し、独立 binary `yoi-workspace-server` として起動できるようにする。
- server binary は少なくとも `serve` subcommand を持つ。
- server binary options:
  - `--workspace <PATH>` / `--workspace=<PATH>`: default cwd。
  - `--db <PATH>` / `--db=<PATH>`: default `<workspace>/.yoi/workspace.db`。
  - `--frontend <PATH>` / `--frontend=<PATH>`: optional static SPA build dir。
  - `--listen <ADDR>` / `--listen=<ADDR>`: default `127.0.0.1:8787`。
  - `--help` / `-h`。
- `yoi workspace serve ...` を追加する。
  - `yoi` crate は `yoi-workspace-server` crate に依存しない。
  - launcher は `YOI_WORKSPACE_SERVER_COMMAND` override または current exe と同じ directory の `yoi-workspace-server` を解決する。
  - launcher は引数を外部 server binary に渡し、終了 status を反映する。
- package build では `yoi` と `yoi-workspace-server` を別 binary として build/install する。
- help に `yoi workspace serve` を表示する。

## 受け入れ条件

- `cargo run -p yoi-workspace-server -- serve --workspace . --db .yoi/workspace.db --listen 127.0.0.1:8787` 相当で server が起動できる。
- `yoi workspace serve ...` が外部 `yoi-workspace-server` binary を起動する。
- `crates/yoi` は `yoi-workspace-server` に依存しない。
- `yoi --help` / `yoi workspace --help` に起動方法が出る。
- `cargo fmt --check`、関連 `cargo test` / `cargo check`、`git diff --check`、`yoi ticket doctor`、`nix build .#yoi --no-link` が通る。
