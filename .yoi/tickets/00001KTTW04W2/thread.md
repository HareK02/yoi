<!-- event: create author: "yoi ticket" at: 2026-06-11T08:15:24Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T10:21:40Z -->

## Intake summary

既存 Ticket 00001KTTW04W2 の body/thread/artifacts と関連 Ticket を確認した。これは implementation_ready。範囲は Orchestrator の progress を Companion に read-only weak notification として渡すことであり、AutoKick / re-kick / scheduler ではない。`Method::Notify` に `auto_run: bool` を追加し、Companion progress notice では `auto_run: false` を使う。`auto_run:false` は idle Pod を起こさず NotifyBuffer に積むだけで、live/reachable Companion への best-effort delivery に限定し、missing/stopped Companion の spawn/restore や初期 persistent snapshot は行わない。通知内容は durable/queryable state から bounded に生成し、history に残らない context-only injection、secret/unbounded log、Companion への mutation/spawn/merge authority 付与は禁止。関連の Companion lifecycle/profile policy は closed 済みで、この Ticket は starvation prevention Ticket 00001KTJXS31R とは非重複の follow-up。blocking open questions はない。risk_flags: [notification-semantics, panel-lifecycle, companion-policy, authority-boundary, prompt-context, persistence, sensitive-content]。

---

<!-- event: state_changed author: intake at: 2026-06-11T10:21:40Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、AutoKick/re-kick との差分、Companion authority 境界、weak notification semantics、bounded/safe context、validation focus が整理され、Orchestrator が routing できる状態になった。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-11T10:31:56Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
