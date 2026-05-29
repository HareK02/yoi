<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:04Z -->

## Migrated

Migrated from tickets/manual-turn-rollback.md. No legacy review file was present at migration time.

---

<!-- event: close author: hare at: 2026-05-29T03:09:22Z status: closed -->

## Closed

---
id: 20260527-000004-manual-turn-rollback
slug: manual-turn-rollback
title: Pod/TUI: 手動 rewind 導線
status: closed
kind: task
priority: P2
labels: [tui, pod, ux]
created_at: 2026-05-27T00:00:04Z
updated_at: 2026-05-29T03:09:22Z
assignee: null
legacy_ticket: tickets/manual-turn-rollback.md
---

## Background

`pod-empty-turn-rollback` / `tui-empty-turn-restore` により、AI 側出力が 0 の interrupted turn については Pod 側で自動 rollback し、TUI 側で入力を復元できるようになった。

次に欲しいのは、直前 turn だけの rollback command ではなく、TUI から過去の user message を選び、その地点まで会話を戻してその入力を composer に復元する **manual rewind** 導線である。

誤送信、モデル選択ミス、途中で方針を変えた場合などに、ユーザーは過去の入力を選び直し、必要なら編集してから Enter で retry できる。選択した瞬間に再実行はしない。

## UX

- `:rewind` command を追加する。
- `:rollback` は `:rewind` の alias として扱ってよい。
- `Ctrl+R` は rewind/rollback を表す shortcut として、同じ picker を開く。
- `:rewind` / `Ctrl+R` は引数を取らず、TUI 内の picker を開く。
- Rewind picker は popup/overlay ではなく、通常の conversation/history view area を一時的に置き換える dedicated view として表示する。
  - composer/input area と actionbar/status area は通常通り残す。
  - main view area だけが message history から rewind target list に切り替わる。
  - Esc 等で picker を閉じると、通常の conversation/history view に戻る。
- `:rewind` command は `Idle` / `Paused` の時だけ picker を開く。`Running` 中は visible diagnostic を出して何もしない。
- `Ctrl+R` shortcut も Pod が停止中 (`Idle` または `Paused`) の時だけ有効にする。`Running` 中は無視または visible diagnostic にする。
- picker は過去の user message を新しい順に表示する。
  - turn number / index
  - timestamp または relative time
  - message preview
  - eligible / disabled reason
- picker で user message を選択すると、Pod はその user message の直前まで history/session log を rewind し、選択された message を TUI composer に復元する。
- 選択後は、composer に該当 message が入っている状態になる。
  - Enter を押すとその message で retry できる。
  - ユーザーは送信前に編集できる。
  - 選択しただけで自動実行しない。
- Esc 等で picker を閉じると何も変更しない。

## Semantics

Manual rewind は destructive operation として扱う。選択地点より後の履歴 suffix は捨てる。fork は優先度低めの別機能であり、この ticket の実装では fork を作らない。

- Rewind は current active segment/session に対して行う。
- Rewind 成功時、選択された `UserInput` entry 自体も履歴から取り除かれ、composer に戻る。
- Rewind 後、選択地点より後の assistant output / later user messages / usage entries / display blocks は現 branch から消える。
- 元 suffix を保持したい場合は将来の `pod-session-fork` で扱う。この ticket では保持しない。
- Tool side effect の undo はしない。

Initial safety policy:

- Pod が `Idle` または `Paused` の時だけ許可する。
- `Running` 中は拒否する。
- picker 表示時から head が変わった場合は apply 時に再検証して拒否する。
- segment rotation / compaction を跨ぐ rewind は初期実装では対象外でよい。
- suffix に tool call / tool result / other side-effect-looking entries が含まれる場合でも、初期方針としては destructive rewind を許可してよい。ただし UI には「以降の履歴は破棄され、tool side effects は undo されない」ことが分かる notice/diagnostic を出す。
- 実装上どうしても安全に整合性を保てない suffix 種別がある場合は、具体的な disabled reason を表示して拒否する。

## Protocol / ownership

TUI がローカルに履歴を削るのではなく、Pod が authoritative に rewind を検証・適用する。

Suggested protocol shape:

```rust
Method::ListRewindTargets { limit: Option<usize> }
Method::RewindTo {
    target: RewindTargetId,
    expected_head_entries: usize,
}

Event::RewindTargets { targets: Vec<RewindTarget> }
Event::RewindApplied {
    entries: Vec<serde_json::Value>,
    input: Vec<Segment>,
    summary: RewindSummary,
}
```

Exact names may differ, but the behavior should stay:

- listing targets and applying a target are separate operations.
- apply revalidates target identity and current head.
- success returns enough entries for clients to reseed their view.
- success returns the selected user input segments so TUI can restore the composer.
- failure uses visible diagnostics, e.g. `Event::Error { code: InvalidRequest, message }`.

`RunResult::RolledBack` should not be reused for this idle control operation. It remains the run-lifecycle signal for submit-time empty-turn rollback.

## Implementation notes

- Target identity can initially be current segment + entry index:

```rust
RewindTargetId {
    segment_id: SegmentId,
    user_input_entry_index: usize,
}
```

- Include `expected_head_entries` to reject stale picker selections.
- Each target should include:
  - preview
  - original `Vec<Segment>`
  - turn/index metadata if available
  - whether the target is eligible
  - disabled/warning reason if relevant
  - the entry count to truncate to, which is before the selected user message.
- Rewind apply must keep these in sync:
  - worker history
  - `user_segments`
  - session store segment log
  - `SegmentLogSink` mirror
  - usage history / trackers
  - TUI view reconstructed from returned entries
- If a complete current-state reconstruction from log is simpler and safer than maintaining many historical snapshots, prefer that over fragile partial truncation.

## Acceptance criteria

- `:rewind` opens a picker of past user messages by replacing the normal conversation/history view area, not by drawing a small popup.
- `Ctrl+R` opens the same picker only while Pod status is `Idle` or `Paused`; it is disabled/rejected while `Running`.
- Selecting a message rewinds the Pod state to before that message and restores the message into the TUI composer.
- Rewind does not auto-run; pressing Enter after selection retries the restored message.
- Rewind success updates Pod session log, SegmentLogSink mirror, worker state, and TUI display consistently.
- Esc returns from the rewind picker to the normal conversation/history view without changing Pod state.
- Rewind failure leaves state unchanged and shows a clear reason.
- Picker selections are revalidated at apply time to avoid stale-head corruption.
- Rewound suffix is intentionally discarded; no fork is created.
- Tool side effects are not undone; UI/diagnostics make this clear when relevant.
- Tests cover target listing, apply success, stale-head rejection, composer restore, TUI display reseed, and at least one suffix-with-tool case.
- `cargo fmt --check`
- `cargo check -p protocol -p pod -p tui`
- Relevant focused tests.

## Out of scope

- Creating a fork when rewinding.
- Fork tree visualization.
- Merging branches.
- Undoing tool side effects.
- Rollback history stack / redo.
- Rewind across compacted segments unless it falls out naturally from implementation.

## Related

- `20260527-000009-pod-session-fork` remains a lower-priority future feature for preserving alternate histories.
- Completed: `pod-empty-turn-rollback`
- Completed: `tui-empty-turn-restore`


---
