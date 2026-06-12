---
title: 'Ticket role launch inputを短縮し、role behaviorをInstruction/Workflowへ分離する'
state: 'done'
created_at: '2026-06-11T16:03:28Z'
updated_at: '2026-06-12T13:08:11Z'
assignee: null
risk_flags: ['prompt-context', 'workflow-boundary', 'role-launch']
queued_by: 'workspace-panel'
queued_at: '2026-06-12T12:29:10Z'
---

## Background

Ticket role Pod launch 時の初回 user message に、role の振る舞いや手順説明が長く含まれている。これを整理し、model-visible な情報の所管を明確にする。

合意済みの分類:

- Instruction: role behavior、恒常的な環境情報、language、workspace_root / cwd など Pod の動作に必要な環境情報。
- Workflow: 手続き、手順、routing / implementation / review / close の flow。
- Submit: 対象 Ticket 指定、ユーザー入力または system-generated action instruction、今回の操作対象として明示する path / worktree / branch / validation / report expectations。
- Control-plane only: profile selector、workflow slug、launch_prompt ref、pod name など。原則として初回 user message には出さない。

関連する既存 Ticket:

- `00001KTRKZ14C`: Project workflowsをpublic builtinとdogfood運用に分離する。closed。
- `00001KTGFMW70`: Builtin Workflow and Knowledge resources。closed。
- `00001KTR6YVDB`: LLM向けプロンプト直書きを廃止してresources/promptsへ集約する。closed。

## Requirements

- Ticket role launch の generated first-run user message から、role behavior / procedural guidance / launch metadata の長文説明を削除する。
- `Profile selector`、`Workflow`、`Configured launch_prompt ref` のような control-plane metadata は、初回 user message に含めない。
- role としての振る舞いは builtin role Profile が選ぶ `worker.instruction` に移す。
- 手続き・手順は Workflow に置く。
- Submit に残す情報は、対象 Ticket id、ユーザー入力または launcher-generated action instruction、今回の操作対象として必要な per-launch context に限定する。
- `language` は Instruction の環境情報として扱う。
- `workspace_root` / `cwd` は Instruction/system 側の環境情報として扱う。ただし Git 操作や worktree 操作の対象として明示が必要な path は、重複しても Submit に書いてよい。
- `resources/prompts/ticket_role/*.md` に残っている role-specific 長文 fragment を削除、縮小、または Instruction/Workflow 側へ移す。
- `TicketRoleLaunchContext.user_instruction` は実態に合わせ、必要なら `action_instruction` / `launch_instruction` などへ rename する。user-authored text と launcher-generated instruction が混ざっていることを表現できる名前にする。
- 既存の workflow invocation、workspace/user prompt override、Ticket role launch、Panel Intake / Orchestrator launch の動作を壊さない。

## Acceptance criteria

- Ticket role launch の first-run `Segment::Text` が短くなり、対象 Ticket / action instruction / per-launch context だけを含む。
- role behavior は builtin role instruction prompt から供給される。
- procedural guidance は Workflow に残る、または Workflow に移されている。
- control-plane metadata は prompt text ではなく launch plan / diagnostics / trace に留まる。
- Ticket record language guidance は初回 user message ではなく Instruction 側の環境情報として扱われる。
- Orchestrator launch で必要な workspace/cwd/topology 情報は、Pod 環境情報と per-operation Submit 情報に分離され、Git/worktree 操作対象として必要なものは Submit 内で明示される。
- 関連する unit tests が新しい出力方針を検証している。
- `nix build .#yoi` が通る。

## Binding decisions / invariants

- Instruction: 振る舞い、恒常的な環境情報。
- Workflow: 手続き、手順。
- Submit: 対象チケットの指定、ユーザー入力または system-generated action instruction、今回の操作対象。
- `workflow slug` と `profile selector` は初回 user message には不要。
- `language` は Instruction 所管。
- `workspace_root` / `cwd` は Instruction/system 側に置く。ただし今回の Git/worktree 操作対象として必要な path は Submit に重複記載してよい。
- Ticket config に role-level `system_instruction` / `instruction` field は追加しない。builtin role Profile が `worker.instruction` を選ぶ。
- launch context template 化は行わない。過剰なテンプレート化ではなく、Rust 側の短い Submit 組み立てに留める。

## Implementation latitude

- builtin role instruction prompt のファイル名・配置は実装者が既存 prompt resource conventions に合わせて決めてよい。
- `resources/prompts/ticket_role` の既存 fragment は、不要なら削除してよい。
- `user_instruction` rename は、変更範囲が大きすぎる場合は別名追加や段階的移行でもよい。ただし model-visible label は `User/action instruction` より正確な名前にする。
- Workflow 側に移す文言は public builtin workflow と workspace dogfood workflow の責務差を保つ。

## Readiness

- readiness: implementation_ready
- risk_flags: [prompt-context, workflow-boundary, role-launch]

## Escalation conditions

- Instruction / Workflow / Submit の境界に収まらない新しい model-visible 情報が見つかった場合は、実装前に判断を戻す。
- Ticket role launch の dynamic data を Instruction minijinja context に入れたくなる場合は、処理対象データを system prompt に入れていないか確認して判断を戻す。
- Ticket config schema に新しい instruction field を追加したくなる場合は判断を戻す。

## Validation

- Ticket role launch prompt 出力を検証する unit tests。
- builtin role Profile が instruction prompt を解決できることの検証。
- relevant cargo tests。
- `nix build .#yoi`。

## Related work

- `resources/prompts/ticket_role/*.md`
- `resources/profiles/{intake,orchestrator,coder,reviewer}.lua`
- `resources/workflows/*`
- `.yoi/workflow/*`
- `crates/client/src/ticket_role.rs`
- `crates/pod/src/prompt/system.rs`
