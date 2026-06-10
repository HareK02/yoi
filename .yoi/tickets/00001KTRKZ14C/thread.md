<!-- event: create author: "yoi ticket" at: 2026-06-10T11:16:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: ticket-intake at: 2026-06-10T11:18:41Z -->

## Plan

## Intake refinement

既存 Ticket を読み、重複作成はしない方針で確認した。この Ticket は loader 実装そのものではなく、現行 project workflow を public builtin 候補と dogfood/project-local 運用へ分離するための設計・監査 work item として扱う。

Readiness classification: `spike_needed` / planning-ready。Orchestrator は実装開始ではなく、まず workflow content audit と source/provenance/priority/resource placement の設計判断を行う planning/spike として route できる。後続で loader 実装や workflow content 移行が必要になった場合は、この Ticket の結論から concrete implementation Ticket に分割する。

Risk flags:

- `prompt-resource`: workflow 文言は LLM-facing prompt resource と同等に扱う必要がある。
- `workflow-source`: builtin / workspace / skill の provenance と priority を誤ると override semantics が曖昧になる。
- `public-api`: public builtin workflow の slug と resident advertisement はユーザー向け contract になり得る。
- `dogfood-boundary`: Yoi repository 固有の Git/worktree/cargo/nix 運用を public default に混ぜない。
- `migration-compat`: `.yoi/ticket.config.toml` の既存 workflow selectors と legacy `ticket-preflight-workflow` 名の扱いを決める必要がある。

Open questions: blocking な追加質問はない。設計中に複数案が残る場合は、workspace override が builtin より優先されること、LLM-facing text を Rust code に直書きしないこと、optional Git/worktree workflow を default resident にしないことを invariant として扱い、未決定点は後続 Ticket 化または decision record 化する。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-10T11:18:55Z -->

## Intake summary

既存 Ticket 00001KTRKZ14C を重複作成せず精査した。対象は現行 `.yoi/workflow/*.md` を public builtin 候補と Yoi repository dogfood/project-local 運用へ分離する設計・監査 work item。Orchestrator は実装開始ではなく planning/spike として route し、workflow content audit、`WorkflowSource::Builtin` 等の provenance、builtin/workspace/skill priority、resident advertisement、resource placement、`.yoi/ticket.config.toml` migration、廃止語彙削除方針を決める。blocking open question はないが、risk flags は prompt-resource / workflow-source / public-api / dogfood-boundary / migration-compat。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-10T11:18:55Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により、Orchestrator が planning/spike routing できる情報が揃った。実装 side effect はまだ開始しない。

---
