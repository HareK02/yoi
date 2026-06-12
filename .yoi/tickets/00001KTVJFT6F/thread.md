<!-- event: create author: LocalTicketBackend at: 2026-06-11T14:48:26Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T14:48:31Z -->

## Intake summary

Panel focus UX の不満を `yoi panel` 専用の concrete Ticket として整理した。合意済み方針は、user-visible focus model を増やさず、composer target を送信先、selected row を空 composer 時の navigation / Enter 対象として扱い、`item action focus` を user-visible focus から外すこと。既存 Ticket / Pod authority と composer 入力保護は維持する。

---

<!-- event: state_changed author: intake at: 2026-06-11T14:48:31Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake clarified the panel focus request and recorded an implementation-ready concrete Ticket. Human can queue it from the panel when ready.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-12T14:44:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-12T14:45:35Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body は `yoi panel` に範囲を限定し、composer target を送信先、row selection を空 composer 時の navigation / Enter 対象として扱う方針を明確に記録している。
- `item action focus` を user-visible focus model から外すこと、composer 入力保護、既存 Ticket / Pod authority の維持、single-Pod TUI navigation を範囲外にすることが binding invariant として記録済みである。
- risk flags は `tui-ux` / `input-safety` だが、bounded context check の結果、具体的な未決定 product/API/authority 判断は残っていない。残る不確実性は `crates/tui/src/multi_pod.rs` 周辺の local tactic / focused tests に閉じている。
- Relation blocker はなく、OrchestrationPlan に accepted plan `orch-plan-20260612-144501-1` を記録済み。現在 inprogress Ticket / child Pod はなく、Orchestrator worktree は clean。

Evidence checked:
- Ticket body / thread: requirements, acceptance criteria, binding decisions, implementation latitude, escalation conditions, validation, intake summary, `ready -> queued` event を確認。
- Related design Ticket: `00001KSKBPPMR` は broad planning context であり、本 Ticket が panel-specific concrete implementation unit であることを確認。
- TicketRelationQuery: outgoing/incoming relation なし、blocker なし。
- TicketOrchestrationPlanQuery: 既存 record なし、今回 accepted plan を記録。
- Code map: `crates/tui/src/multi_pod.rs` に `PanelFocus::ItemAction`, `focus_item_action`, `Right action focus` hints, focus/status/actionbar rendering, key handling, focused tests があることを確認。
- Workspace/Pod state: Orchestrator worktree `orchestration/yoi-orchestrator` は clean、visible child Pod なし、inprogress Ticket なし。
- Durable context: Panel composer target は selected rows とは別の送信先で、Panel は authority/backend ではなく local view/controller に留める既存方針に一致する。

IntentPacket:

Intent:
- `yoi panel` の user-visible focus model を composer target と row selection に整理し、実際の Enter/矢印/Esc/Tab 挙動と status/actionbar/key hints を一致させる。
- `item action focus` / `Right action focus` が user-visible focus として見える状態を削除または実際に意味のある表示に変える。

Binding decisions / invariants:
- 対象は `yoi panel` のみ。single-Pod TUI transcript / block navigation は範囲外。
- composer に文字がある場合は通常入力と composer draft 保護を最優先する。
- composer target は focus ではなく送信先である。
- row selection は空 composer 時の navigation / Enter 対象である。
- `item action focus` を user-visible focus model から外す。
- Panel は durable state authority にならず、Ticket / Pod authority と `ready -> queued` 等の明示 user action semantics を変えない。

Requirements / acceptance criteria:
- Panel 上で composer target と selected row の意味が分かりやすく表示される。
- `global composer` / `selected row` / `item action` のように実際の操作以上の focus 状態を見せない。
- 空 composer と非空 composer で Enter が何をするか、status/actionbar から誤解しにくい。
- `↑/↓`, `Left`, `Right`, `Esc`, `Tab`, `Enter` の key hints が実装と一致する。
- `Right action focus` が残る場合は user-visible focus ではなく意味ある操作として説明する。不要なら削除・無効化する。
- composer 入力保護を維持する。
- Ticket action dispatch / Pod open / Intake launch / Companion send の安全性を落とさない。

Implementation latitude:
- `PanelFocus` の状態名・数・遷移は変更してよい。
- `ItemAction` 相当の状態は削除してよい。
- `Right` / `Left` は削除、no-op、案内表示のいずれでもよいが、hints と一致させる。
- `Esc` の詳細挙動は input safety を満たす範囲で調整してよい。
- UX 改善は最小実装でよく、フル navigation mode / vim-like 操作体系は不要。

Escalate if:
- composer 入力保護と row navigation の優先順位について記録済み invariant を超える判断が必要になった場合。
- Ticket lifecycle action の明示性や authority boundary を変える必要が出た場合。
- single-Pod TUI navigation model 変更が必要だと判明した場合。

Validation:
- `crates/tui/src/multi_pod.rs` 周辺の focused tests を追加・更新する。
- focus/key handling/status/actionbar の unit tests を通す。
- `cargo test -p tui multi_` またはより focused な同等テスト。
- `cargo fmt --check`。
- `git diff --check`。
- `./result/bin/yoi ticket doctor` または同等。
- `nix build .#yoi`。

Current code map:
- 主対象: `crates/tui/src/multi_pod.rs`。
- 既存確認点: `PanelFocus`, `focus_item_action`, `effective_focus`, key handling around `Up/Down/Left/Right/Enter/Tab/Esc`, status/actionbar rendering, row hints, tests around `multi_`.

Critical risks / reviewer focus:
- composer draft を誤って submit / clear / row action dispatch しないこと。
- key hints と実装がずれないこと。
- `Right action focus` / `item action` 表示が残留して UX を曖昧にしないこと。
- Ticket action dispatch、Pod open、Intake launch、Companion send の authority semantics が変わっていないこと。
- broad `00001KSKBPPMR` の single-Pod transcript/block navigation scope に踏み込んでいないこと。

Next action:
- `queued -> inprogress` を記録してから、branch `ticket/panel-focus-composer-row-selection` / worktree `/home/hare/Projects/yoi/.worktree/panel-focus-composer-row-selection` を作成し、sibling coder に narrow write scope で実装を委譲する。Reviewer は coder evidence 後に read-only で起動する。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T14:45:41Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing accepted for implementation. Ticket body/thread, related broad design Ticket, relation blockers, orchestration plan, code map, current Orchestrator workspace state, in-progress Ticket state, and visible Pods were rechecked. No unresolved blocker or missing planning decision remains. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded, using accepted plan `orch-plan-20260612-144501-1`.

---

<!-- event: plan author: orchestrator at: 2026-06-12T14:46:00Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTVJFT6F`
- Branch: `ticket/panel-focus-composer-row-selection`
- Worktree: `/home/hare/Projects/yoi/.worktree/panel-focus-composer-row-selection`
- Base: Orchestrator workspace HEAD `f13ab294` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Scope rule: implementation work happens only in this child worktree. Root/original workspace must not be read, written, validated, merged, cleaned, or used for git operations.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---
