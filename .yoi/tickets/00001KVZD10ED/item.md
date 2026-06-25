---
title: 'llm-worker crateをllm-engineへ改名する'
state: 'inprogress'
created_at: '2026-06-25T12:45:38Z'
updated_at: '2026-06-25T13:47:26Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T13:24:26Z'
---

## 背景

今後は旧 `Pod` 相当の実行単位を `Worker` として扱い、`Runtime` が複数 Worker を保持・操作する構造へ移行したい。一方、現在の `crates/llm-worker` は実行単位としての Worker ではなく、LLM provider request / stream parsing / tool-call loop / reasoning / usage / retry / continuation / low-level history を進める turn engine である。

このまま `llm-worker::Worker` という名前を残すと、今後導入する `worker-runtime::Worker` / Runtime scoped Worker identity と衝突し、Pod から Worker への概念移行が分かりにくくなる。責務分離自体は現在の `llm-worker` のままで概ね良いが、名前は実体に合わせて `llm-engine` へ変更する。

この Ticket では crate rename、Rust module path rename、主要型 rename、関連 procedural macro crate rename を一括で行う。中途半端に crate 名だけ変えると check が通りにくく、移行中の混乱も残るため、`llm-worker -> llm-engine`、`llm-worker-macros -> llm-engine-macros`、`Worker -> Engine` を同じ実装単位で完了させる。

## 要件

### Crate / package rename

- `crates/llm-worker` を `crates/llm-engine` に rename する。
- `crates/llm-worker-macros` を `crates/llm-engine-macros` に rename する。
- Cargo package name を `llm-worker` から `llm-engine` に変更する。
- Cargo package name を `llm-worker-macros` から `llm-engine-macros` に変更する。
- Workspace `Cargo.toml`、crate dependencies、imports、tests、docs、Nix packaging references を新しい crate 名に更新する。
- Rust import path は `llm_worker` から `llm_engine` に変更する。
- Macro crate import path は `llm_worker_macros` から `llm_engine_macros` に変更する。
- 旧 crate 名 compatibility alias は作らない。

### Type / API rename

- `llm_worker::Worker` を `llm_engine::Engine` に rename する。
- `WorkerConfig` は `EngineConfig` に rename する。
- `WorkerResult` は `EngineRunResult` または同等に rename する。
- `WorkerError` は `EngineError` に rename する。
- `RunOutput` は `EngineRunOutput` または `RunOutput` のままでもよいが、public API 上で `Worker` という語が turn engine の主体名として残らないよう整理する。
- `ToolDefinition as WorkerToolDefinition` のような import alias は、意味が残るなら `EngineToolDefinition` 等に更新する。
- 内部 doc comment / examples / tests の「Worker」は、実行単位としての Worker と混同しないよう `Engine` / `LLM engine` / `turn engine` に更新する。

### Responsibility boundary

- `llm-engine` は Runtime / Worker identity / process lifecycle / socket protocol / session file authority を持たない。
- `llm-engine` は LLM turn engine として以下を担う。
  - provider request / stream handling
  - normalized LLM events
  - tool-call loop
  - text / reasoning / tool / usage handling
  - retry / continuation
  - low-level history management
  - callback hooks
- `pod` crate や将来の `worker-runtime` crate が、実行単位としての Worker lifecycle / Runtime scoped identity / transcript projection / API exposure を担う。
- この Ticket では責務移動は最小限にし、主に naming / package boundary を整理する。

### Migration scope

- Repository-wide references to `llm-worker`, `llm-worker-macros`, `llm_worker`, and `llm_worker::Worker` are updated.
- Repository-wide references to `llm_worker_macros` are updated to `llm_engine_macros`.
- `pod` crate uses `llm_engine::Engine` internally.
- Tests / fixtures / generated docs that mention the old crate/type names are updated.
- If generated lock/package files change, they are updated consistently.
- Obsolete paths are removed; no duplicate `crates/llm-worker` or `crates/llm-worker-macros` directory remains.

## Non-goals

- New `worker-runtime` crate implementation.
- Pod -> Worker runtime migration.
- Backend internal Companion implementation.
- Runtime network API implementation.
- Moving session persistence / socket protocol / metadata authority into `llm-engine`.
- Changing provider request semantics, tool-call loop behavior, retry/continuation behavior, or history semantics beyond rename fallout.
- Backward compatibility alias for the unreleased `llm-worker` crate name.

## 受け入れ条件

- `crates/llm-engine` and `crates/llm-engine-macros` exist; `crates/llm-worker` and `crates/llm-worker-macros` are gone.
- Cargo package names and crate import paths are `llm-engine` / `llm_engine` and `llm-engine-macros` / `llm_engine_macros`.
- Public turn-engine type is `llm_engine::Engine`, not `llm_engine::Worker`.
- Public config/result/error names no longer use `Worker` for the LLM turn engine concept.
- Repository-wide references to `llm-worker` / `llm-worker-macros` / `llm_worker` / `llm_worker_macros` are gone except migration notes or changelog-like context where intentionally retained.
- `pod` and dependent crates compile against `llm_engine::Engine`.
- Existing behavior of provider streaming, tool-call loop, history append callbacks, retry/continuation, and usage events is unchanged except for names.
- Docs / comments that define the new WorkerRuntime direction no longer conflict with the LLM engine naming.
- `cargo test -p llm-engine` passes.
- `cargo test -p pod` passes.
- `cargo check -p yoi` passes.
- `git diff --check` passes.
- `nix build .#yoi --no-link` passes.
