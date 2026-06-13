<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:03Z -->

## Migrated

Migrated from tickets/internal-worker-workflow.md. No legacy review file was present at migration time.

---

<!-- event: plan author: ticket-intake at: 2026-06-13T09:25:59Z -->

## Plan

## Intake refinement

既存 Ticket `00001KSKBPAXR` の body / thread / artifacts を確認した。artifacts は `.gitkeep` のみで、thread は migration 記録のみだった。重複 Ticket は作成しない。

### 現状整理

この Ticket は legacy migration 時点の前提を多く含んでいる。

- 旧名 `INSOMNIA`、旧 path `.insomnia/workflow/<slug>.md`、旧 `tickets/*.md` 参照が残っている。
- その後、Workflow / prompt resource 境界の設計は更新されている。
  - `00001KTRKZ14C` は closed。public builtin workflow と Yoi dogfood workflow の分離、`resources/workflows/<slug>.md`、`WorkflowSource::Builtin`、workspace override、resident core/optional 方針を記録済み。
  - `00001KTGFMW70` は closed。embedded builtin Workflow resources、Workflow-required builtin Knowledge fallback/provenance、workspace override を実装済み。
  - 現在の internal prompt は `resources/prompts/internal/{memory_extract_system,memory_consolidation_system,compact_system}.md` と `PromptCatalog` / `resources/prompts/internal.toml` 側で扱われている。

### Intake 判断

現時点で、この Ticket を元のまま「内部 Worker / 内部 Pod を Workflow と同一仕様で実行する」実装 Ticket として route するのは危険。現在の設計では、Workflow は手続き・procedural flow、Prompt resources は system prompt / role behavior / internal worker prompt を所有する別 boundary であり、両者を混ぜると prompt-context / workflow-boundary / tool authority の責務が曖昧になる。

したがって readiness は `requirements_sync_needed`。Orchestrator に渡す前に、人間/maintainer が次のいずれかを選ぶ必要がある。

1. **退役 / superseded 扱い**: この legacy Ticket は `00001KTRKZ14C`、`00001KTGFMW70`、および現在の `PromptCatalog` internal prompt resource 化で実質的に置き換えられたとして、Orchestrator/human が close する。
2. **PromptCatalog follow-up へ retarget**: Workflow 化ではなく、internal worker prompt の remaining gap を concrete に切り直す。例: extract / consolidation / compact の workspace/user/prompt-pack override、provenance diagnostics、test coverage、docs の不足確認。
3. **真の internal Workflow 呼び出し substrate を新設**: 既存の Prompt resource / Workflow boundary を変更する設計 Ticket として再定義する。この場合は、なぜ PromptCatalog では不足か、tool surface 表明を workflow frontmatter に載せる authority model をどう安全にするか、`user_invocable: false` と resident/launch provenance をどう扱うかを先に設計判断する必要がある。

### Binding decisions / invariants for any refinement

- Workflow prose、Prompt fragments/internal prompts、Knowledge records は別 resource boundary として扱う。混ぜる場合は明示的な設計判断が必要。
- 内部 Worker prompt を model-visible context に載せる場合も、turn を跨ぐ volatile hidden injection にならないよう、既存の history / prompt context 原則に従う。
- `resources/prompts` にある internal prompt は PromptCatalog の責務であり、Workflow loader の責務へ silently 移さない。
- `resources/workflows` の builtin workflow は procedural flow の resource であり、Yoi dogfood semantics を public builtin slug に隠さない。
- `INSOMNIA` / `.insomnia` / legacy `tickets/*.md` 参照は current Ticket routing 前に Yoi / `.yoi` / canonical Ticket ID へ読み替えまたは整理する。

### Risk flags / reviewer focus

- `prompt-context`
- `workflow-boundary`
- `runtime-resource`
- `tool-authority`
- `memory-prompt`
- `migration-compat`

### Open question

この Ticket は退役させるか、PromptCatalog follow-up に切り直すか、internal Workflow substrate の新設設計として再定義するか。現時点ではこの人間判断がないため、`ready` にはしない。

---

<!-- event: state_changed author: hare at: 2026-06-13T09:56:34Z from: planning to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-13T09:56:34Z status: closed -->

## 完了

## Resolution

ユーザー指示により close する。

この Ticket は legacy migration 由来の「内部 Worker / 内部 Pod を Workflow と同一仕様で扱う」構想だったが、現在の設計では Workflow と Prompt resource / internal prompt は別 boundary として整理されている。

- public builtin workflow / Yoi dogfood workflow の分離、`resources/workflows/<slug>.md`、workspace override、builtin provenance は関連 Ticket で対応済み。
- internal prompt は `resources/prompts/internal/*` と `PromptCatalog` / `resources/prompts/internal.toml` 側の責務として扱う。
- 元の要件に残る `INSOMNIA`、`.insomnia/workflow`、旧 `tickets/*.md` 前提は current Yoi 設計と一致しない。

したがって、この Ticket は実装 routing せず、退役 / superseded として完了扱いにする。将来、internal prompt の remaining gap や internal Workflow substrate が必要になった場合は、現在の Prompt resource / Workflow boundary を前提にした別の concrete Ticket として作成する。

---
