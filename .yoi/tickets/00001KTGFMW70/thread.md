<!-- event: create author: LocalTicketBackend at: 2026-06-07T07:27:08Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: intake at: 2026-06-11T07:56:24Z -->

## Plan

## Intake refinement: builtin Workflow / Knowledge resource implementation

この Ticket は新規作成せず、既存 Ticket `00001KTGFMW70` を実装可能な follow-up として更新する。関連する設計 Ticket `00001KTRKZ14C` は closed で、workflow 側の builtin source / provenance / source priority / public-vs-dogfood split の判断を提供しているため、本 Ticket はその実装 follow-up として扱う。

### Clarified scope

- builtin Workflow resource と builtin Knowledge resource を runtime に同梱し、workspace に同名ファイルが無くても参照できるようにする。
- workspace/project 側の同名 record は builtin を override できる。override / fallback は provenance とともに観測可能にする。
- Workflow は `resources/workflows/<slug>.md` を builtin resource 置き場とする。これは `00001KTRKZ14C` / `docs/design/workflows-public-dogfood-split.md` の決定に従う。
- Knowledge は同じ resource boundary の発想で `resources/knowledge/<slug>.md` または既存の resource loader 構成に自然に収まる同等の明示的ディレクトリを使う。prompt fragment と混同しないこと。
- resident / advertised workflow の集合は小さく保つ。core builtin は `ticket-intake-workflow` と `ticket-orchestrator-routing`、optional builtin は `multi-agent-workflow`、compatibility-only は `ticket-preflight-workflow` とする。Yoi repository dogfood workflow は public builtin semantics を同名で shadow しない。

### Acceptance criteria

- builtin Workflow / Knowledge resource が embedded runtime resources として package build 後にも読める。
- workspace/project resources が builtin より優先され、同名 override 時も source/provenance が list/show/launch/context で確認できる。
- source priority は workflow design document の判断を壊さない: explicit path/file selector がある場合は最優先、workspace/project、user source が既存または明示的に実装される場合は user、builtin、explicitly enabled plugin/skill source の順を基本にする。
- Workflow launch context には、どの workflow text が使われたかを示す source/provenance が残る。
- Knowledge context でも builtin 由来か workspace/user override 由来かを reviewer が追える。
- `resources/prompts` に workflow/knowledge prose を混在させない。
- tests は source priority、workspace override、missing workspace file fallback、package/resource availability の少なくとも主要経路を覆う。
- runtime resource / packaging に触れるため、完了前に `nix build .#yoi` または同等の flake package build を通す。

### Binding decisions / invariants

- Prompt context に hidden injection しない。Workflow / Knowledge resource を LLM context に載せる場合は、既存の history / worker context 原則に従い、turn を跨いで根拠が消える形にしない。
- Workflow prose、Knowledge records、Prompt fragments は別の resource boundary として扱う。
- Builtin resource の存在は workspace files の authority を奪わない。workspace override は明示的かつ provenance-visible にする。
- Dogfood workflow semantics を public builtin slug に隠して載せない。Yoi 固有 Git/worktree/cargo/nix/merge policy は workspace-local dogfood slug または launch context 側に残す。
- Secret / credential / private prompt を builtin Knowledge として同梱しない。

### Implementation latitude

- `WorkflowSource::Builtin` そのもの、または同等の typed provenance を実装してよい。
- Workflow と Knowledge で共有 resolver を作るか、各 crate に薄く分けるかは実装判断でよい。ただし source priority / provenance の振る舞いは揃える。
- user-level resource directory が既存実装に無い場合、まず workspace override + builtin fallback を完成させ、user source は明示的な config/storage boundary が必要なら小さく実装するか、別 follow-up に切る判断を Orchestrator にエスカレーションしてよい。

### Risk flags / reviewer focus

- risk_flags: `[runtime-resource, workflow-source, knowledge-source, provenance, prompt-context, packaging]`
- Reviewer は loader/resolver の優先順位、provenance 表示、prompt context 原則、`resources/prompts` との混同、Nix package resource inclusion を重点確認する。

### Escalation conditions

- Knowledge 側に source-qualified selector / user-level override の public API を新設する必要があり、既存 Memory/Knowledge schema や Profile/manifest authority を変える場合。
- Workflow slug / selector syntax を変更し、既存 `/workflow` invocation や `.yoi/ticket.config.toml` の workflow selector に互換影響が出る場合。
- Builtin resource inclusion が Nix packaging / install layout に影響し、compile-time embed 以外の配布方式が必要になる場合。

### Readiness

- readiness: `implementation_ready`
- blocking open questions: none
- related: `00001KTRKZ14C`, `docs/design/workflows-public-dogfood-split.md`


---

<!-- event: intake_summary author: intake at: 2026-06-11T07:56:31Z -->

## Intake summary

既存 Ticket 00001KTGFMW70 を重複作成せず更新した。これは closed Ticket 00001KTRKZ14C の workflow source/provenance/design 判断を受けた実装 follow-up として扱う。scope は embedded builtin Workflow / Knowledge resources、workspace/project override、provenance-visible lookup/launch/context、tests、packaging validation。blocking open question はない。risk flags / reviewer focus は runtime-resource / workflow-source / knowledge-source / provenance / prompt-context / packaging。Orchestrator は implementation_ready として queue 後に routing できるが、Knowledge user-source public API や selector syntax 変更が必要になった場合は escalation。

---

<!-- event: state_changed author: intake at: 2026-06-11T07:56:31Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、要件・invariants・implementation latitude・risk flags・validation focus が揃ったため、Orchestrator が routing できる状態として ready にする。実装 side effect はまだ開始しない。

---
