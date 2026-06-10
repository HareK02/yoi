---
title: 'Project workflowsをpublic builtinとdogfood運用に分離する'
state: 'ready'
created_at: '2026-06-10T11:16:30Z'
updated_at: '2026-06-10T11:18:55Z'
assignee: null
---

## 背景

現在の workflow は workspace-local な `.yoi/workflow/*.md` として管理されている。対象は以下。

- `ticket-intake-workflow.md`
- `ticket-orchestrator-routing.md`
- `ticket-preflight-workflow.md`
- `multi-agent-workflow.md`
- `worktree-workflow.md`

これらには Yoi の public builtin workflow として有用な Ticket / Pod orchestration 原則が含まれている一方、この repository の dogfooding 運用に強く結合した Git / worktree / branch / commit / merge / cargo / nix / `.yoi` sparse checkout などの具体手順も混ざっている。

そのまま builtin 化すると、公開用の標準 workflow が Yoi 開発 repository 固有の運用をユーザー project に押し付けることになる。builtin 化の前に、public default として提供する汎用 workflow と、この repository での dogfooding workflow を明確に分離する必要がある。

## 現状分析

- Workflow loader は主に `<workspace>/.yoi/workflow/<slug>.md` を読む。
- `WorkflowSource` には現在 `WorkspaceWorkflow` と `Skill { dir }` があり、`Builtin` source はない。
- Pod 起動時に workspace workflows を読み、その後 manifest の `[skills] directories` 由来の `SKILL.md` を registry に merge している。
- resident workflow advertisement は workflow metadata に基づくため、builtin 化した workflow は model context への露出範囲も設計する必要がある。
- 現在の `.yoi/ticket.config.toml` は project-local workflow slug を role に紐づけている。
  - Intake: `ticket-intake-workflow`
  - Orchestrator: `ticket-orchestrator-routing`
  - Coder/Reviewer: `multi-agent-workflow`

## 問題意識

- `ticket-intake-workflow.md` は builtin 化しやすいが、古い `Action required` / `Attention required` 語彙や Yoi dogfooding 寄りの表現を整理する必要がある。
- `ticket-orchestrator-routing.md` は builtin 化候補だが、Git/worktree/branch など実装隔離 mechanism への具体言及を薄め、workspace state / active workspaces / isolation mechanism といった抽象表現へ寄せる必要がある。
- `ticket-preflight-workflow.md` は内容としては planning / requirements sync に近く、public-facing slug と既存互換名の扱いを決める必要がある。
- `multi-agent-workflow.md` は Yoi dogfooding 色が強く、`yoi を yoi で開発する`、worktree + branch + commit + merge + cargo/nix/yoi doctor などが濃い。そのまま public builtin にしない。
- `worktree-workflow.md` は Git worktree 専用であり、optional な Git workflow としてはあり得るが、default resident workflow にするべきではない。
- LLM-facing workflow 文言は prompt の一種なので、builtin workflow の resource placement は `resources/prompts` 集約方針と整合させる必要がある。

## 要件

- 現行 project workflows を audit し、public builtin に抽出する内容と dogfood/project-local に残す内容を分離する。
- public builtin workflow に入れるべき抽象原則を整理する。
  - Ticket は durable orchestration record。
  - Task は session-local progress tracking。
  - Objective は中期 goal / decision context。
  - Intake は scheduler ではない。
  - Ticket 作成前に user agreement を取る。
  - broad effort を umbrella/progress-container Ticket にしない。
  - binding decisions / invariants と implementation latitude を分ける。
  - risk flags は automatic stop gate ではない。
  - `queued -> inprogress` は side effect 前の explicit acceptance。
  - notification は proof ではない。
  - coder/reviewer は sibling として扱う。
  - reviewer は intent / invariant / acceptance criteria に照らして判断する。
  - merge-ready dossier / handoff report の考え方。
  - secrets/private context を records に残さない。
  - prompt context / authority boundary に触れる変更は escalation。
- public builtin workflow から、この repository 固有の dogfooding 手順を外す。
  - `yoi を yoi で開発する` 前提。
  - `.worktree/<task-name>` 固定。
  - `git worktree add` / sparse checkout 除外リスト。
  - branch naming / commit / merge / cleanup の細部。
  - `TODO.md` / `docs/report` / `.yoi/memory` 固有運用。
  - `cargo fmt --check` / `cargo check --workspace` / `cargo test` / `nix build` / `yoi ticket doctor` の固定 validation。
- dogfood workflow はこの repository 用の project-local workflow として残す。
  - public builtin と同じ slug で override するか、`yoi-dogfood-*` のような明示 slug に分けるかを判断し、理由を記録する。
- builtin workflow source model を設計する。
  - `WorkflowSource::Builtin` または同等の provenance を追加する方針を決める。
  - builtin workflow の synthetic path / source label / diagnostics 表示を決める。
- workflow source priority / override semantics を決める。
  - workspace `.yoi/workflow/<slug>.md` が builtin を override できること。
  - skill workflows と builtin workflows の優先順位を明示すること。
- resident advertisement の分類を決める。
  - core builtin resident workflow と optional builtin workflow を分ける。
  - Git/worktree workflow を default resident にするかどうかを明示する。
- builtin workflow resource placement を決める。
  - LLM-facing text 集約方針に従い、`resources/prompts/workflows/` など prompt resource 配下に置く案を検討する。
  - workflow crate の record/source model と矛盾させない。
- `.yoi/ticket.config.toml` の workflow selector を、分離後の slug / source model に合わせて更新する。
- stale な `Action required` / `Attention required` workflow 文言を削除・置換する。

## 推奨する分解案

### Public builtin core workflows

- `ticket-intake-workflow`
- `ticket-orchestrator-routing`
- `ticket-planning-sync` または既存互換の `ticket-preflight-workflow`

### Public builtin optional workflows

- `multi-agent-review-workflow`
- `git-worktree-isolation-workflow`

optional workflows は builtin resource として提供しても、default resident advertisement には含めない、または profile/feature で明示有効化する。

### Project dogfood workflows

- `yoi-dogfood-multi-agent-workflow`
- `yoi-dogfood-worktree-workflow`

この repository の concrete Git/worktree/cargo/nix/Ticket doctor 運用は dogfood workflow 側に残す。

## 受け入れ条件

- 現行 `.yoi/workflow/*.md` の内容が、public builtin 候補と dogfood/project-local 残置対象に分類されている。
- public builtin にそのまま入れてはいけない Git/worktree/Yoi-repo-specific 文言が audit され、移動先または削除理由が記録されている。
- `WorkflowSource::Builtin` など builtin workflow source/provenance の設計が記録されている。
- source priority が明文化されている。
  - workspace override が builtin より優先されること。
  - skill workflow と builtin workflow の優先順位。
- resident advertisement の core/optional 方針が決まっている。
- builtin workflow resource を `resources/prompts` 配下に置くか、別 resource tree に置くかの判断と理由が記録されている。
- public builtin workflow の slug と dogfood workflow の slug 方針が決まっている。
- `.yoi/ticket.config.toml` をどう移行するかが記録されている。
- `Action required` / `Attention required` のような廃止済み Ticket frontmatter 語彙が workflow から取り除かれる計画になっている。
- 後続実装 Ticket に分割できる粒度まで設計が落ちている。
- `target/debug/yoi ticket doctor` が通る。

## 非目標

- この Ticket 単体で builtin workflow loader を実装すること。
- この Ticket 単体ですべての workflow content を書き換えて移行完了すること。
- Git/worktree workflow を public default resident workflow として無条件に有効化すること。
- Yoi dogfooding 固有の運用を捨てること。
- workflow 文言を Rust code に直書きすること。
