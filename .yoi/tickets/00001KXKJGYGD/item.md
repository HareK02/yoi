---
title: 'Remove workflow tracking and workflow resources'
state: 'inprogress'
created_at: '2026-07-15T19:02:13Z'
updated_at: '2026-07-15T21:35:26Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-15T19:54:36Z'
---

## 背景

現在の Workflow は、Workflow resource を single-step な procedural prompt として提示し、ActiveWorkflowList / ActiveWorkflowComplete / ActiveWorkflowCancel で追跡する中途半端な obligation tracking になっている。今後の再利用可能な作業手順は別 Ticket で扱う Agent Skills 形式へ寄せるため、この Ticket では既存 Workflow tracking / resource / invocation path の削除に範囲を限定する。

外部状態や authority は Workflow ではなく typed feature/tool surface が持つ。Ticket queue、Worker spawn、workdir 管理、review wait などの制御を Workflow に移さない。

## 要件

- ActiveWorkflow tracking 系を削除する。
  - `ActiveWorkflowList` / `ActiveWorkflowComplete` / `ActiveWorkflowCancel` tools を削除する。
  - Worker durable state、snapshot、compaction、rehydration、system-item extension から active workflow snapshot / obligation を削除する。
  - resident workflow / active workflow prompt wording を削除する。
- Workflow resource / invocation path を削除する。
  - `resources/workflows` / `.yoi/workflow` を長期 authority として扱わない。
  - `/workflow-slug` 形式の invocation を削除する。
  - Workflow 関連 crate/resource/API/test を削除する。
- 既存 role prompts / internal prompts / docs から Workflow 固有の記述を削除する。
- Workflow 削除後も Ticket / Worker / workdir / queue などの外部状態制御は typed feature/tool surface に残す。
- 既存 session に ActiveWorkflow storage が残っている場合の扱いを明示する。
  - 初期実装では無視 / drop / diagnostic のいずれかを決め、migration code を増やしすぎない。

## 受け入れ条件

- `ActiveWorkflow*` tools が model-visible tool schema から消えている。
- active workflow state が Worker snapshot / compaction / rehydration に残らない。
- Workflow resource discovery / invocation path が削除されている。
- Workflow 関連 prompt wording / resident workflow advertisement が消えている。
- Workflow 関連 crate/resource/API/test が削除されている、または不要な互換 layer なしに整理されている。
- Ticket queue、Worker spawn、workdir 管理などの外部状態制御は Workflow ではなく feature/tool surface に残る。
- affected crates の `cargo test` と `nix build .#yoi` が通る。
