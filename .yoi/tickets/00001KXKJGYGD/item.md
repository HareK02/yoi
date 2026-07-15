---
title: 'Replace workflow tracking with proper Skills support'
state: 'ready'
created_at: '2026-07-15T19:02:13Z'
updated_at: '2026-07-15T19:27:11Z'
assignee: null
---

## 背景

現在の Workflow は、Workflow resource を single-step な procedural prompt として提示し、ActiveWorkflowList / ActiveWorkflowComplete / ActiveWorkflowCancel で追跡する中途半端な obligation tracking になっている。Workflow を state machine や外部状態接続の制御機構へ育てると、Ticket queue 消化、Worker spawn、workdir 管理、review wait などは結局 Plugin / feature tool surface の責務になり、Workflow が半端なスクリプト言語になってしまう。

一方で、Yoi には既に Skills/SKILL.md 的な資源のサポートが部分的にあり、LLM に渡す再利用可能な手順・作法・報告形式は Skill として扱う方が自然。外部 authority や状態変更は typed feature/tool 側に置き、Skill は参照可能な procedural guidance としてちゃんとサポートする。

## 要件

- ActiveWorkflow tracking 系を削除する。
  - ActiveWorkflowList / ActiveWorkflowComplete / ActiveWorkflowCancel tools を削除する。
  - Worker durable state / compaction / rehydration / system-item extension から active workflow snapshot を削除する。
  - active workflow prompt / resident workflow obligation wording を削除または Skill 前提に置き換える。
- 既存 Workflow resource の扱いを廃止または Skill 互換へ移行する。
  - `resources/workflows` / `.yoi/workflow` の長期 authority をやめる。
  - 必要な既存 reusable 手順は `SKILL.md` 形式へ移す。
  - workflow invocation `/slug` の扱いは削除するか、明示的な Skill invocation / Skill reference に置き換える。
- Skills を first-class にする。
  - builtin skill と workspace skill の discovery / override / provenance を定義する。
  - Skill は `SKILL.md` を最小形とし、LLM-facing procedural guidance として扱う。
  - Skill は外部状態を直接制御しない。Ticket / Worker / workdir / queue などの authority は feature/tool surface が持つ。
- Knowledge と Skill の境界を明確にする。
  - Knowledge は facts / rationale / reference。
  - Skill は task execution guidance / reusable procedure / report shape。
- Plugin / feature との境界を明確にする。
  - Skill は prompt/resource。
  - Plugin / feature は typed tools、external state、authority、automation を提供する。
- Coder review cycle や Orchestrator queue consumption は、Skill + Workspace/Ticket/Worker feature tools の組み合わせで設計できるようにする。
- 既存 role prompts から Workflow 固有の記述を削除し、Skill を使う文言へ更新する。
- Workflow 関連 crate/resource/API/test を削除または Skill 実装へ置き換え、不要な互換 layer を残さない。

## 受け入れ条件

- `ActiveWorkflow*` tools が model-visible tool schema から消えている。
- active workflow state が Worker snapshot / compaction / rehydration に残らない。
- Workflow resource discovery / invocation path が削除されるか、Skill discovery / invocation に置き換わっている。
- builtin/workspace Skill の loading、override priority、provenance、lint が tests で確認されている。
- 既存 workflow resources のうち残すべき手順は Skill に移行されている。
- role prompts / internal prompts / docs が Workflow 前提ではなく Skill 前提に更新されている。
- Ticket queue、Worker spawn、workdir 管理などの外部状態制御は Workflow/Skill ではなく feature/tool surface に残る。
- affected crates の `cargo test` と `nix build .#yoi` が通る。
