<!-- event: create author: "yoi ticket" at: 2026-06-15T06:08:19Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T06:37:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T06:38:25Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relation / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- 対象は single-Pod conversation View の text-like Item selection/copy に限定され、Panel row selection とは scope が分かれている。
- Tool/non-text Item と copy target は implementation-time decision として許容されており、binding invariants / acceptance criteria / escalation conditions が明確。

Evidence checked:
- Ticket body/thread: requirements、copy target、tool/non-text handling、acceptance criteria、non-goals、related work を確認。
- Ticket relations: `00001KV072V89` と `00001KV10SN02` は related のみで blocker ではない。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`f0de8413` 上。
- Visible Pods: implementation child Pod なし。Intake peer は routing blocker ではない。
- Bounded code map: `crates/tui/src/single_pod.rs`、copy/clipboard abstraction、mouse/selection/key handling tests。

IntentPacket:

Intent:
- single-Pod TUI conversation View で User/System/Assistant など text-like Item の表示テキストを mouse drag で選択し、`y` で copy、`Esc` で clear できるようにする。

Binding decisions / invariants:
- 対象は single-Pod View Item text。Panel Ticket/Pod row selection は変更しない。
- terminal-native text selection preservation は non-goal。View drag は Yoi text selection として扱う。
- selected/copy text、selection state、clipboard diagnostics は Pod history / model context / session log / Ticket records に残さない。
- composer input、scroll、rewind picker、modal/popup、normal key handling と衝突させない。
- bare Panel row mouse selection semantics は regress させない。
- Tool/non-text Item handling と copy target は実装報告または decision comment に明示する。

Requirements / acceptance criteria:
- UserItem / SystemItem / AssistantItem text を drag selection できる。
- drag start/update/release 後に selection state が残り、View 上で highlight される。
- `Esc` で selection clear。
- `y` で selected text を copy し、selection clear。
- text-like Item を跨いだ selection が deterministic separator で copy される。
- non-text/tool item handling decision が明示され、test で固定される。
- copy 成功/失敗が user-visible で、secret-like diagnostics を出さない。
- tests cover coordinate mapping、selection state、multi-item extraction、Esc、copy+clear、non-text/tool handling。

Implementation latitude:
- Existing clipboard abstraction があれば利用。なければ testable な最小 copy path を追加してよい。
- OSC52/system clipboard/internal copy buffer の選択は既存 TUI architecture に合わせる。
- rendering/highlight 表現、selection model の internal shape、separator は実装側判断。ただし deterministic にする。

Escalate if:
- terminal/crossterm event stream で必要な drag/release を識別できない。
- existing rendering model から text coordinate mapping を安全に取れない。
- copy target を追加すると secrets/history/model-context boundary に影響する。
- rewind/modal/composer key handling と共存できない大きな model change が必要。

Validation:
- focused `cargo test -p tui ...` for single_pod selection/copy。
- `cargo check -p tui --all-targets`。
- `cargo fmt --check`。
- `git diff --check`。
- Panel row mouse selection regression test if related mouse plumbing is touched。

Critical risks / reviewer focus:
- selection/copy state の persistence leakage。
- mouse coordinate -> rendered text mapping correctness。
- multi-item extraction ordering/separator。
- composer/scroll/rewind/modal conflict。
- clipboard path safety and testability。
- Panel mouse behavior regression absence。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T06:39:16Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、single-Pod View work は同時に開始する Panel composer work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
