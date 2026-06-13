<!-- event: create author: LocalTicketBackend at: 2026-06-13T09:23:07Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-06-13T10:00:19Z -->

## Plan

## 調査・修正計画

ユーザー合意により、実装時はまず一時 diagnostic を入れて live 挙動を確認し、原因を確定してから本修正する方針とする。問題が解消したら一時ログは外して完了する。

### Phase 1: 一時 diagnostic の追加

`Event::RewindApplied` 周辺と rewind picker submit 周辺に、秘密情報を含まない短い一時ログまたは TUI diagnostic を入れる。

確認する値:

- `Event::RewindApplied` が `Enter` 直後に処理されるか、`Esc` 後まで遅れるか。
- `Event::RewindApplied` 処理時の `App::greeting.is_some()`。
- `restore_snapshot(&entries, greeting)` を呼べているか。
- `entries.len()`。
- `rewind_picker.is_some()` / applying 状態。
- `input.is_empty()` と composer restore 分岐。
- `pod_status`。

### Phase 2: live 再現確認

一時 diagnostic 入りの binary で、既知の手順を再現する。

1. TUI 起動。
2. `Ctrl+R` で rewind targets を開く。
3. target を選択して `Enter`。
4. 画面が無反応なら少し待つ。
5. `Esc` で戻る。
6. diagnostic から、以下のどれに該当するか判断する。

判断観点:

- `RewindApplied` が `Enter` 直後に処理され、`greeting=false` なら、live TUI が rewind 後 `entries` を restore できず stale 表示になっている可能性が高い。
- `RewindApplied` が `Esc` 後まで処理されないなら、event loop / socket delivery / wake-up 側を主因として追う。
- `RewindApplied` が `Enter` 直後に処理され、`greeting=true` かつ restore 済みなら、draw / overlay / picker close / scroll state の問題を疑う。

### Phase 3: 本修正

原因に応じて修正する。

- `App::greeting` 欠落で restore が skip されている場合:
  - `RewindApplied` restore failure を silent success にしない。
  - `greeting` を失う経路を修正するか、`RewindApplied` を self-contained にする、または fresh snapshot request / explicit diagnostic の妥当な方針を実装する。
- picker 中に Pod event 処理が遅れる場合:
  - single-pod TUI event loop / `PodClient` wake-up / drain ordering / connection gating を修正し、Pod event で即時 redraw されるようにする。
- restore は動いているが表示が stale の場合:
  - `restore_snapshot()` 後の picker close、draw、scroll、overlay state を修正する。

### Phase 4: 二重 submit 防止

主因修正とは別に、rewind picker submit 後の `Enter` 連打を防ぐ。

- `RewindPickerState` に applying/pending 状態を持たせる、または同等の idempotency guard を追加する。
- submit 後は追加 `Enter` で複数 `Method::RewindTo` を生成しない。
- 必要なら `Applying rewind...` のような状態表示を出す。
- 成功 / failure / rejection で pending を解除または picker を閉じる。

### Phase 5: focused test と一時ログ削除

- 原因に対応する focused regression test を追加する。
- `Event::RewindApplied` で live TUI transcript が巻き戻し後 `entries` に reseed され、picker が閉じることを確認する。
- metadata 欠落時に silently skip しないことを確認する。
- pending 中の追加 `Enter` が複数 `Method::RewindTo` を生成しないことを確認する。
- composer restore 分岐を確認する。
- live 確認で問題が解消したら、一時 diagnostic / debug log を削除する。

### Validation

- focused tests
- `cargo fmt --check`
- `cargo check -p protocol -p pod -p tui`

この計画は、原因未確定のまま protocol/schema 変更へ飛ばず、まず live diagnostic で `greeting` 欠落・event 処理遅延・表示更新不整合のどれかを切り分けることを重視する。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-13T10:02:13Z -->

## Intake summary

ユーザー依頼を `00001KV04NJ8D` として具体化し、read-only 調査結果と追加観測を反映した。Pod 側 rewind は成功しているが live TUI が `Event::RewindApplied` の反映または snapshot restore を即時実行できていない可能性を主仮説として記録済み。合意済み計画として、一時 diagnostic を入れて live 再現で `RewindApplied` timing / `App::greeting` / `restore_snapshot()` / picker pending を切り分け、原因修正後に一時ログを外し、focused tests と `cargo fmt --check` / `cargo check -p protocol -p pod -p tui` で検証する。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-13T10:02:13Z from: planning to: ready reason: planning_ready field: state -->

## State changed

ユーザーが Ticket の ready 化を明示したため、Orchestrator が routing できる状態にする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T10:53:20Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T10:56:29Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、症状、既知の関連実装、受け入れ条件、調査範囲、実装 latitude、escalation conditions が具体化されている。
- `TicketRelationQuery` と `TicketOrchestrationPlanQuery` で blocker / ordering / conflict 記録は見つからなかった。
- 関連の closed Ticket `00001KSKBPBX0` は rewind picker / RewindTo flow の既存実装背景として確認済みで、本 Ticket はその follow-up bugfix として独立に扱える。
- risk flags は `tui-state` / `rewind` / `stream-sync` だが、Ticket は rewind 成功時に live 表示を snapshot/remaining session に同期する invariant と、generation id / reload を含む実装 latitude を明記しており、実装前に不足する設計判断はない。
- 現 Orchestrator worktree は clean。root/original workspace では git/read/write/validate せず、実装は専用 child worktree に隔離する。
- 主な変更面は single-Pod rewind / app state / Pod RewindTo response path で、Panel mouse selection Ticket `00001KV072V89` の panel row hit-test surface とは分離できるため並列開始候補にする。

Evidence checked:
- Ticket body / thread / artifacts（artifacts なし）。
- relation records: なし。
- orchestration plan records: なし。
- related Ticket `00001KSKBPBX0` の intent / prior rewind picker scope。
- code map: `crates/tui/src/single_pod.rs` の rewind picker/UI flow、`crates/tui/src/app.rs` の app state / event handling、`crates/pod/src/**` の `RewindTo` / `RewindApplied` path、`crates/protocol` の response type 周辺。
- workspace/Pod state: Orchestrator worktree clean、visible live implementation Pods なし。

IntentPacket:

Intent:
- rewind picker で Enter により RewindTo が成功した後、TUI live 表示が巻き戻し後の session tail / snapshot 状態へ確実に同期されるようにする。

Binding decisions / invariants:
- RewindTo の成功通知だけを見て cosmetic reload するのではなく、表示 state と Pod/session state の整合を保つ。
- rewind は破壊的 state operation なので、古い generation / stale stream / stale pending reload が live 表示を再汚染しないこと。
- TUI-local state fix を優先し、Pod の永続 session model / history authority を不必要に変更しない。
- 未完了 run や stream 中の rewind を勝手に許可しない。既存 idle/control constraints を尊重する。

Requirements / acceptance criteria:
- rewind picker Enter 後、成功した rewind target より後の old output / live tail が残らない。
- 成功後の composer/status/actionbar が既存 UX と矛盾しない。
- no-op / cancelled / failed rewind では表示を誤って消さない。
- stale stream/update が rewind 後の表示を復活させない。
- focused tests で rewind success / failure / stale update などを確認する。

Implementation latitude:
- Pod response に既存情報で足りるなら TUI 側 reload/generation 管理で直す。
- 既存 protocol が足りない場合は最小の typed response 拡張を検討してよいが、protocol/API の互換境界を変える必要がある場合は escalation する。
- UI reload のタイミング、generation id、snapshot再取得、buffer clear のどれを使うかは bounded investigation に委ねる。

Escalate if:
- protocol/API の public contract を大きく変える必要がある。
- rewind の history authority / persisted session semantics を変更しないと直せない。
- stream中 rewind許可や concurrent run semantics の設計判断が必要になる。
- fix が broad TUI event-loop rewrite を要求する。

Validation:
- focused TUI/app rewind tests、必要なら Pod protocol/unit tests。
- `cargo fmt --check`。
- `git diff --check`。
- 変更範囲に応じて `cargo test -p tui` / `cargo test -p pod` / `cargo check --workspace --all-targets`。

Current code map:
- `crates/tui/src/single_pod.rs`: rewind picker input/display flow。
- `crates/tui/src/app.rs`: session item/live state、pending reload、generation/stream handling の候補。
- `crates/pod/src/**` and `crates/protocol/**`: `RewindTo` / `RewindApplied` response path and typed metadata。

Critical risks / reviewer focus:
- 成功 rewind 後に old assistant output / live tail が残らないこと。
- failed/cancelled rewind の no-op semantics。
- stale stream/reload ordering。
- persisted history authority を TUI 側都合で歪めていないこと。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T10:56:45Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Ticket evidence、relations、orchestration plan、関連 closed Ticket、bounded code map、Orchestrator worktree clean state を確認した。blocking relation はなく、rewind live refresh の要件・invariants・escalation conditions は実装可能な粒度で記録済み。実装 side effect の前に routing decision / IntentPacket / accepted plan を記録したため、専用 child worktree と Coder delegation に進む。

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T10:59:38Z -->

## Implementation report

Implementation handoff started.

- Worktree: `/home/hare/Projects/yoi/.worktree/rewind-live-refresh`
- Branch: `ticket-00001KV04NJ8D-rewind-live-refresh`
- Coder Pod: `coder-00001KV04NJ8D-rewind`
- Scope: child worktree read, `crates/tui` / `crates/pod` / `crates/protocol` write, this Ticket record write。`.yoi/memory` や local/runtime/log/lock/secret-like `.yoi` paths は write scope に含めていない。
- Note: 初回 SpawnPod は runtime launch validation が workspace root `/home/hare/Projects/yoi` の readable grant を要求して失敗したため、非再帰 read grant のみ追加して再実行した。Coder には root/original workspace を inspect/write/git/validate/merge/cleanup しないよう明示済み。

Next:
- Coder の commit / implementation_report / validation evidence を確認後、Reviewer を read-only 基本で起動する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T11:14:26Z -->

## Implementation report

Implementation report for commit 949ceb5a (fix: refresh tui after rewind)

Files changed:
- crates/tui/src/app.rs: added rewind apply-pending state, post-rewind live-update fence, authoritative RewindApplied restore path that always clears/replays retained entries, actionbar-visible failure handling, and focused rewind refresh tests.
- crates/tui/src/single_pod.rs: route Esc through cancel_rewind_picker so submitted rewinds stay visibly pending until the Pod responds.
- crates/tui/src/ui.rs: show an applying/waiting state in the rewind picker header while RewindTo is pending.

Validation:
- cargo test -p tui rewind_refresh_tests: PASS (4 tests).
- cargo test -p tui single_pod::tests::rewind_picker: PASS (2 tests).
- cargo fmt --check: PASS.
- git diff --check: PASS.
- cargo test -p tui: ATTEMPTED; unrelated failures remain in multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace and spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}. The rewind-focused tests above passed.

Residual risks:
- The stale-update fence is intentionally short-lived and clears on authoritative Status/Snapshot after RewindApplied; it drops queued display-mutating live events during that window to avoid re-polluting the restored session tail.
- No protocol or Pod persistence changes were made.

---

<!-- event: review author: hare at: 2026-06-13T11:21:32Z status: approve -->

## Review: approve

## Review: approve

Reviewed implementation commits `949ceb5a` and `3a7edbde` against Ticket `00001KV04NJ8D` intent and acceptance criteria.

Evidence:
- `Event::RewindApplied` no longer gates transcript restoration on `App::greeting`; it clears/replays the Pod-provided post-rewind entries through a shared restore path and emits an explicit warning if greeting metadata is unavailable, avoiding silent stale-view success.
- Rewind picker submit now enters an `applying` state, suppresses repeated `Enter`/navigation, shows an applying header, blocks `Esc` from hiding an in-flight destructive request, and closes on successful `RewindApplied`.
- Rewind failure (`Event::Error` while applying) clears the pending state, leaves the existing transcript intact, and surfaces an actionbar-visible failure plus normal error block.
- A short rewind refresh fence drops display-mutating stale live events after successful restore until authoritative `Status`/`Snapshot`; no Pod protocol or persistence semantics changed.
- Temporary investigation logging was not present in the final diff.
- Focused tests cover successful restore/old-tail removal, missing-greeting restore, duplicate-submit suppression with failure preservation, and stale live update suppression; existing rewind picker tests still pass.

Validation performed:
- `git diff --check 20daae0c..HEAD`: PASS.
- `cargo test -p tui rewind_refresh_tests`: PASS (4 tests).
- `cargo test -p tui single_pod::tests::rewind_picker`: PASS (2 tests).
- `cargo fmt --check`: PASS.
- `cargo check -p protocol -p pod -p tui`: PASS.
- `cargo test -p tui`: FAILED only in the already-reported unrelated tests: `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace` and `spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}`; rewind-focused tests passed in that run.

Residual note:
- The stale-update fence intentionally relies on the Pod's follow-up `Status`/`Snapshot` to clear; this matches the current `RewindApplied`/`Status` flow and is acceptable for this Ticket.

Decision: approve.

---
