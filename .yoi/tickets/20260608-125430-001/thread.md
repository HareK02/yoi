<!-- event: create author: LocalTicketBackend at: 2026-06-08T12:54:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-09T00:17:06Z -->

## Plan

## Intake refinement

既存 Ticket は `Objective` record の目的・背景・要件・受け入れ条件をすでに含んでおり、新規 Ticket 作成ではなくこの Ticket の refinement で足りる。`objective` label の重複 Ticket は見つからず、関連する planning Ticket として `typed-ticket-relation-metadata` と `deprecate-umbrella-tickets` がある。

### Binding decisions / invariants

- `Objective` は Ticket / OrchestrationPlan / Pod session claim と別の first-class project record として扱う。
- Objective は medium-term goal、motivation、strategy / design direction、success criteria / exit conditions、decision context、linked Tickets を保持するための軽量 record であり、実装可能 work item である Ticket を置き換えない。
- Objective と Ticket の link は dependency / blocking relation ではない。Ticket が Objective に属していても、その Objective 内の他 Ticket すべてに依存することを意味しない。
- Objective context は Intake / Planning / Orchestrator の判断材料になるが、実装判断の authority は引き続き各 Ticket body/thread と明示的な Ticket relation / OrchestrationPlan record にある。
- 初期設計は Markdown-oriented かつ local file backend 前提に留め、roadmap scheduling、milestones、OKR、dependency graph solving、全 Ticket への Objective 必須化は範囲外にする。
- `Project` / `Initiative` ではなく `Objective` という用語を使う。

### Implementation latitude

- 具体的な record schema、`.yoi/` 配下の path、frontmatter fields、CLI/API 名、Panel 表示の段階分けは実装時の調査で提案してよい。
- 初回 scope は設計文書と最小実装方針の提示で足りる。必要であれば小さな local-file reader/writer/API skeleton を提案してよいが、full roadmap system に広げない。
- Ticket 側に Objective reference を置くか、Objective 側に linked Tickets を列挙するか、または両方をどう扱うかは、validation と git-trackable record と運用の単純さを基準に設計してよい。
- `deprecate-umbrella-tickets` と `typed-ticket-relation-metadata` の内容は関連 context として読むべきだが、それらの実装をこの Ticket に取り込まない。

### Escalation conditions

- Objective が dependency/blocking relation、OrchestrationPlan runtime plan、または local Pod/session/worktree claim を兼ね始める設計になりそうな場合は Orchestrator/人間判断へ戻す。
- Objective を全 Ticket に必須化する、または scheduling / roadmap / OKR system に拡張する必要が出た場合は scope creep として戻す。
- public CLI/API、storage migration、backward compatibility、Panel UX の binding decision が複数案で割れる場合は、設計案と trade-off を thread に記録して判断を戻す。
- Ticket body/thread を読まずに Objective context だけで実装判断できるような導線になりそうな場合は、Ticket authority boundary に反するため戻す。

### Readiness

- readiness: implementation_ready
- risk_flags: [project-record, ticket-model, workflow, persistence, cli-api, panel-ux]
- blocking open questions: none

Validation は既存 acceptance criteria の通り `target/debug/yoi ticket doctor` と `git diff --check` を含める。runtime resource / prompt / packaging / code に触れる実装になった場合は repository guidance に従って focused tests と `nix build .#yoi` も実施する。

---

<!-- event: intake_summary author: intake at: 2026-06-09T00:17:13Z -->

## Intake summary

既存 Ticket を新規作成せず refinement した。`Objective` は Ticket / OrchestrationPlan / Pod/session claim と別の lightweight project record とし、medium-term goal・motivation・strategy/design direction・success criteria/exit conditions・decision context・linked Tickets を保持する。Ticket を置き換えず、Objective link は dependency/blocking relation ではない。初期 scope は Markdown/local-file 前提の設計と最小実装方針に限定し、roadmap scheduling、milestones、OKR、dependency graph solving、全 Ticket への Objective 必須化は範囲外。blocking open questions はなく、Orchestrator が implementation/design work として routing できる。

---

<!-- event: state_changed author: intake at: 2026-06-09T00:17:13Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement により目的・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions が揃ったため、workflow_state を ready にする。

---
