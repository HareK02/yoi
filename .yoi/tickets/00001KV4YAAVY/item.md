---
title: 'single-Pod View Item text をマウスドラッグで選択・コピーできるようにする'
state: 'closed'
created_at: '2026-06-15T06:08:19Z'
updated_at: '2026-06-15T14:59:40Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui', 'mouse-input', 'selection', 'clipboard', 'single-pod-view']
queued_by: 'workspace-panel'
queued_at: '2026-06-15T06:37:00Z'
---

## Background

single-Pod TUI の View では、端末ネイティブ選択に頼らず、Yoi 自身が View Item の表示テキストをマウスドラッグで選択できるようにする。

既存の `00001KV072V89` は `yoi panel` の View item / row click selection であり、この Ticket の対象ではない。この Ticket は Panel ではなく、通常の single-Pod conversation View に表示される Item text の選択・コピーを対象にする。

端末ネイティブ選択は不要。View 内でドラッグしたら自動的に Yoi の selection state に入り、`Esc` で解除、選択したまま `y` で copy & selection clear する。

## Requirements

- 対象は single-Pod TUI の View Item text。
- 主対象 Item は text-like なもの。
  - UserItem
  - SystemItem
  - AssistantItem
- View 内の text-like Item 上で mouse drag すると、自動的に selection state に入る。
  - drag start が anchor
  - drag current/end が focus
  - mouse release 後も selection は保持される
- 選択状態は View 上で highlight される。
- `Esc` で selection を解除する。
- selection がある状態で `y` を押すと、選択テキストを copy し、selection を解除する。
- Item を跨いだ selection を可能にする。
  - text-like Item 間を跨ぐ selection は supported behavior とする。
  - Item 間の copied text separator は読みやすく deterministic にする。基本は newline / blank line のどちらかを実装時に選び、test で固定する。
- 選択 text は model context / Pod history / session log / Ticket に記録しない。
- composer 入力、scroll、rewind picker、modal/popup、通常 key handling と衝突しない。
- View に mouse selection を妨げる mode は基本的に置かない。
  - terminal-native text selection preservation はこの Ticket の goal ではない。
  - View の drag 操作は Yoi text selection として扱う。
- 既存 Panel row click selection と混同しない。
  - Panel row selection behavior を変更しない。
  - single-Pod View Item text selection のみを対象にする。

## Tool / non-text Item handling

Tool 系 Item を selection range に含んだ場合のコピー仕様は未決定であり、実装前または実装中に明示判断する。

候補:

- tool系 Item を non-selectable gap として skip する
- tool系 Item は summary/placeholder text のみコピーする
- tool系 Item に selection が入ったら range boundary をそこで止める

この Ticket の必須範囲は、UserItem / SystemItem / AssistantItem の text-like Item selection である。Tool 系の扱いで設計判断が必要になった場合は、実装報告または decision comment に記録する。

## Copy target

`y` の copy target は実装時に既存 TUI/clipboard abstraction を確認して選ぶ。

優先:

- 既存 clipboard abstraction があればそれを使う。
- 無ければ最小の copy path を追加する。

OSC52 / system clipboard / internal copy buffer の選択は実装時に判断してよいが、以下を満たすこと:

- user-visible に copy 成功/失敗が分かる
- secret-like diagnostics を出さない
- selected text が model/history に混入しない
- tests で copy された text を検証できる

## Acceptance criteria

- single-Pod View の UserItem / SystemItem / AssistantItem text を mouse drag で選択できる。
- 選択範囲が View 上で highlight される。
- mouse release 後も selection が保持される。
- `Esc` で selection が解除される。
- `y` で selected text が copy され、selection が解除される。
- text-like Item を跨いだ selection が deterministic な text として copy される。
- Tool 系 Item を range に含んだ時の扱いが、実装または decision comment で明示されている。
- composer / scroll / rewind picker / modal / normal key handling の既存挙動が壊れない。
- Panel row mouse selection には regression がない。
- selection/copy state は Pod history、model context、session log、Ticket records に残らない。
- Focused tests cover:
  - mouse coordinate -> View Item text point mapping
  - drag start/update/release selection state
  - multi-item text selection extraction
  - Esc clear
  - y copy + clear
  - non-text/tool item handling decision
- Validation: focused `cargo test -p tui ...`, `cargo fmt --check` or `cargo fmt -p tui`, `cargo check -p tui --all-targets`, and `git diff --check`.

## Non-goals

- Panel Ticket/Pod row selection.
- Terminal-native text selection preservation.
- Generic terminal scrollback selection.
- Full rich text/HTML/markdown semantic selection.
- Double-click word selection / triple-click line selection.
- Selection in every TUI widget.
- Tool item rich output copy semantics beyond the explicit decision made for this Ticket.

## Related work

- `00001KV072V89` — Workspace panel の View item をマウスで選択できるようにする。Panel row click selection; not this Ticket's scope.
- `00001KV10SN02` — E2E critical path / mouse behavior coverage.
- `crates/tui/src/single_pod.rs` — single-Pod TUI View / mouse handling area.
