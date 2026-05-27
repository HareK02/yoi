<!-- event: create author: tickets.sh at: 2026-05-27T19:44:21Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: orchestrator at: 2026-05-27T19:44:43Z -->

## Plan

## Background

Pod notification / notice によって child Pod の完了や状態変化が見えても、現状の assistant はユーザーから明示的に「レビューして」「確認して」と言われるまで自発的に消化しないことがある。

AGENTS.md や workflow に multi-agent の運用は書かれているが、これは知識として読めるだけで、Pod 管理ツールが利用可能な turn における runtime 行動規範としては弱い。特に、自分が spawn した child Pod の完了通知は background signal として扱い、自然な区切りで `ReadPodOutput` / worktree status / diff / test を確認して次の action に進むべきである。

一方で、notification は non-blocking であり、進行中の user request を不必要に中断してまで消化すべきではない。system instruction には「自発的に follow-up するが、現在の user task を壊さない」というバランスを明示する必要がある。

## Requirements

- Pod management tools が有効な Worker にだけ、Pod orchestration 用の system guidance を注入する。
  - 例: `SpawnPod` / `ReadPodOutput` / `SendToPod` / `StopPod` / `AttachOrRestorePod` などが利用可能な場合。
  - Pod 管理 tool がない通常 Worker / child Pod には不要な guidance を出さない。
- guidance 本文は `resources/prompts` 配下に置く。
  - prompt 文字列を Rust code に直書きしない。
- guidance には以下を含める。
  - Pod notification / notice は、自分が処理すべき background signal として扱う。
  - 自分が spawn した child Pod の完了通知を受けたら、自然な区切りで `ReadPodOutput` を確認する。
  - 委譲 task が完了していれば、報告・worktree status・diff・test 結果を確認し、修正依頼 / merge / ticket 完了処理 / Pod 停止のいずれかに進む。
  - user が明示的に follow-up を要求するまで routine follow-up を放置しない。
  - ただし進行中の user request を不用意に中断しない。
  - output / diff / test を確認せずに完了扱いしない。
- この guidance は scheduler / auto-maintainer ではない。
  - workflow を勝手に開始しない。
  - project decision / merge / cleanup は既存 workflow と user authorization に従う。
- notification / PodEvent を context に載せる場合は、既存の history 永続化原則を破らない。
  - turn を跨げない情報を history に残さず system context にだけ差し込まない。

## Acceptance criteria

- Pod management tools が有効な Worker の system prompt に orchestration guidance が含まれる。
- Pod management tools が無効な Worker には含まれない。
- prompt 本文が `resources/prompts` にある。
- prompt assembly の test で conditional inclusion が確認されている。
- guidance が user request の中断を促さず、natural stopping point での follow-up を促す文言になっている。
- `cargo fmt --check` と関連 crate の test が通る。

## Out of scope

- 自動 scheduler / auto-maintain loop の実装。
- PodEvent / notification の protocol 変更。
- spawned Pod registry restore の修正。
- TUI notification UI の変更。


---
