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
