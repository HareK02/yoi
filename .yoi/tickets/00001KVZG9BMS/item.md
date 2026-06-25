---
title: 'pod crateをworker crateへ改名する'
state: 'inprogress'
created_at: '2026-06-25T13:42:37Z'
updated_at: '2026-06-25T14:16:23Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T14:13:35Z'
---

## 背景

Yoi は旧 `Pod` 相当の実行単位を今後 `Worker` として扱い、`Runtime` が複数 Worker を保持・操作する構造へ移行する。既存の `crates/pod` は、現在の `yoi pod` process / Unix socket / session persistence / tool registry / workflow / `llm-engine` turn execution host を束ねる実行単位の本体であり、実質的には「single Worker host」である。

`llm-worker` は `llm-engine` に改名して LLM turn engine から `Worker` 名を空ける。その後、`pod` crate を rewrite ではなく rename として `worker` crate に寄せる。大規模な責務移動はこの Ticket では行わず、名前と公開 API を今後の Runtime/Worker model に合わせる。`pod-store` / `pod-registry` 相当の責務は、この rename Ticket では直接扱わず、後続の `worker-runtime` 作成時に Runtime 内部 persistence / allocation module として吸収する。

## 目的

- 旧 `Pod` 実行単位を `Worker` として命名し直す。
- `worker-runtime` 導入前に、`Worker` が実行単位、`llm-engine` が turn engine、`Runtime` が Worker を動かす環境、という命名を揃える。
- 既存 `pod` crate の実装は基本 rewrite せず、rename / import path / type name / docs / tests を整理する。
- 後続の `worker-runtime` crate が `worker` crate を single Worker host として扱える状態にする。

## 要件

### Crate / package rename

- `crates/pod` を `crates/worker` に rename する。
- Cargo package name を `pod` から `worker` に変更する。
- Rust import path を `pod` から `worker` に変更する。
- Workspace `Cargo.toml`、crate dependencies、tests、docs、Nix packaging references を更新する。
- 旧 crate name / import path の compatibility alias は作らない。

### Type / module rename

- Public type / module / doc comment の `Pod` terminology を Worker terminology に更新する。
  - `Pod` -> `Worker`。
  - `PodConfig` / `PodController` / `PodState` / `PodEvent` 相当があれば Worker terminology に寄せる。
  - `pod_name` は `worker_name` または `worker_id` 相当へ寄せる。
- ただし、既存 socket protocol / session file / on-disk compatibility の詳細に残る `pod` 文字列については、後続 Runtime 移行で消すべき legacy detail として明示的に扱う。
- `llm-engine` 内部の `Engine` と、実行単位としての `worker::Worker` が名前上衝突しないようにする。

### CLI / process surface

- 現在の `yoi pod` CLI surface をどう rename するかを実装時に決める。
- 後方互換は設けなくてよいが、dogfooding runtime / spawn path / scripts / tests への影響を明示的に処理する。
- Low-level process launch path は、後続の `worker-runtime` では compatibility / process-backed Worker host として扱えるようにする。

### Responsibility boundary

- この Ticket は rename が主目的であり、大規模な responsibility rewrite はしない。
- `worker` crate は当面 single Worker host として以下を保持する。
  - input handling
  - `llm-engine` integration
  - event emission
  - session / transcript compatibility
  - tool registry / workflow integration
  - legacy socket compatibility
- Runtime に属する責務は後続 Ticket へ残す。
  - 複数 Worker 管理
  - embedded / networked Runtime API
  - worker store / allocation integration
  - remote runtime support
- `pod-store` / `pod-registry` の standalone rename は行わず、後続 `worker-runtime` 作成時に直接内部 module へ吸収する。

## Non-goals

- `worker-runtime` crate の実装。
- Backend internal Companion Web Console の実装。
- `pod-store` / `pod-registry` の `worker-store` / `worker-registry` への中間 rename。
- Runtime 内部 persistence / allocation module の完成。
- Existing process/socket/session model の完全削除。
- `llm-engine` rename の実装。
- Provider request / tool-call loop / history semantics の変更。

## 受け入れ条件

- `crates/worker` が存在し、`crates/pod` は残っていない。
- Cargo package name and Rust import path are `worker`.
- Public execution-unit type is `worker::Worker`, not `pod::Pod`.
- Repository-wide active references to `pod` crate / `pod::` import / `crates/pod` are gone except intentionally documented legacy context.
- Dependent crates compile against `worker` crate.
- `llm-engine::Engine` と `worker::Worker` の責務境界が code/docs/comments 上で明確になっている。
- Existing process/socket/session compatibility path still works or is explicitly updated without old-name alias.
- `pod-store` / `pod-registry` are not renamed to standalone `worker-store` / `worker-registry` in this Ticket.
- `cargo test -p worker` passes.
- `cargo test -p yoi` or relevant CLI tests covering process launch pass.
- `cargo check -p yoi` passes.
- `git diff --check` passes.
- `nix build .#yoi --no-link` passes.
