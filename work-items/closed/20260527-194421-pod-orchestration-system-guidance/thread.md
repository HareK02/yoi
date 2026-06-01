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

<!-- event: comment author: hare at: 2026-06-01T00:57:03Z -->

## Comment

## Supplemental guidance from dogfooding

Add two explicit rules to the Pod orchestration/system guidance:

- A spawned Pod completion notification is delivered as a normal background signal. The parent does not need to keep the turn open or call tools solely to wait for it; it is acceptable to finish the current turn and handle the notification at the next natural point.
- Do not use `sleep`/polling loops just to wait for a Pod's output. If there is no other useful immediate work, return control to the user instead of blocking the turn; when the notification arrives, read the Pod output then.

Rationale: during multi-agent work, waiting with `sleep` wastes the active turn and fights the notification model. The desired behavior is notification-driven follow-up, not artificial polling.


---

<!-- event: plan author: hare at: 2026-06-01T01:10:57Z -->

## Plan

## Preflight classification

implementation-ready.

The ticket affects prompt/system guidance and conditional prompt assembly, but the desired product behavior is already specified in the ticket thread and item: include orchestration guidance only when Pod management tools are available, keep the prose in `resources/prompts`, and explicitly avoid `sleep`/polling or turn-blocking waits for child Pod output.

## Current code map

- `resources/prompts/`: prompt text sources; new guidance text should live here.
- Prompt assembly code/tests: locate the system prompt construction path that already conditionally includes memory/workflow/tool guidance and add a tool-availability gate for Pod orchestration guidance.
- Tool registry / available tool list: use existing tool availability rather than hard-coding a Worker kind if possible.
- Existing prompt assembly tests: add inclusion/exclusion coverage for Pod management tools available/unavailable.

## Requirements / invariants

- Guidance is conditional on Pod management tools being available.
- Guidance is not shown to Workers without Pod management tools.
- Guidance must not imply an auto scheduler or unauthorized workflow start.
- Guidance must say notifications can be handled at the next natural point and the parent need not keep the turn open just to wait.
- Guidance must say not to use `sleep`/polling loops merely to wait for Pod output.
- Do not change PodEvent/notification protocol, TUI notification UI, spawned registry restore, or workflow semantics.

## Escalate if

- The only available hook requires injecting notification-derived context without durable history.
- Conditional tool-availability detection would require broad ToolRegistry redesign.
- The implementation would change runtime notification delivery or Pod lifecycle behavior instead of prompt guidance.

## Validation

- Focused prompt assembly tests for conditional inclusion/exclusion.
- Any touched crate tests relevant to prompt loading/assembly.
- `cargo fmt --check`.


---

<!-- event: review author: hare at: 2026-06-01T01:24:27Z status: approve -->

## Review: approve

External reviewer recommendation: approve.

Summary:
- Static Pod orchestration guidance was added under `resources/prompts/common/pod-orchestration.md` and registered through the prompt catalog.
- The guidance is appended to the materialized system prompt only when available tool names include Pod-management capabilities.
- The gate uses registered tool definitions, not Worker kind, matching the ticket boundary.
- The prompt explicitly covers background notifications, natural stopping points, not keeping a turn open solely to wait, no `sleep`/polling loops for Pod output, evidence-before-completion, and no scheduler/authorization bypass.

Intent / requirement mapping:
- Included when Pod management tools are enabled: satisfied.
- Omitted when Pod management tools are disabled: satisfied.
- Prompt body lives under `resources/prompts`: satisfied.
- Conditional prompt assembly tests exist: satisfied.

Invariant check:
- No changes to PodEvent/notification protocol, TUI notification UI, spawned registry restore, Pod lifecycle behavior, scheduler/auto-maintain behavior, or notification-derived context injection.
- The implementation adds static guidance based on durable tool availability, not transient notification state.

Blockers: none.

Non-blocking follow-ups:
- A future small test could pin the intended "any Pod-management tool is enough" semantics with a single representative tool.
- Tool-name class recognition could be centralized later if more prompt gates need it.

Reported validation from coder was considered sufficient:
- `cargo test -p pod pod_orchestration`
- `cargo test -p pod prompt::catalog`
- `cargo fmt --check`


---

<!-- event: close author: hare at: 2026-06-01T01:24:59Z status: closed -->

## Closed

Merged and completed.

Implementation:
- Added resource-backed Pod orchestration guidance at `resources/prompts/common/pod-orchestration.md`.
- Registered the guidance through the prompt catalog and internal prompt resources.
- Added conditional system prompt assembly based on registered Pod-management tool names.
- Guidance is included for Workers with Pod management tools and omitted otherwise.
- Guidance explicitly says Pod notifications are background signals handled at natural stopping points, that the parent does not need to keep a turn open solely to wait, and that agents should not use `sleep`/polling loops just to wait for Pod output.
- Guidance also preserves evidence-before-completion and no scheduler/authorization-bypass constraints.

Review:
- External reviewer approved with no blockers.

Validation after merge:
- `cargo test -p pod pod_orchestration` passed.
- `cargo test -p pod prompt::catalog` passed.
- `cargo fmt --check` passed.
- `./tickets.sh doctor` passed.


---
