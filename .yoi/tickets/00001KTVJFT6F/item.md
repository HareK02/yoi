---
title: 'Workspace panel の focus model を composer target と row selection に整理する'
state: 'inprogress'
created_at: '2026-06-11T14:48:26Z'
updated_at: '2026-06-12T15:01:54Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-ux', 'input-safety']
queued_by: 'workspace-panel'
queued_at: '2026-06-12T14:44:16Z'
---

## Background

`yoi panel` の現行 focus 表示は `global composer` / `selected row` / `item action` の3状態だが、実際の操作モデルは composer 入力状態と selected row に強く依存している。

特に `item action focus` は Enter の挙動を実質的に変えないため、ユーザーには「focus が移った」ように見える一方で、何が変わったのか分かりづらい。Panel の主な操作対象は composer target と selected row であり、focus 概念がそれ以上に増えることで UX が不明瞭になっている。

関連する broad design Ticket として `00001KSKBPPMR`「TUI: navigation mode / block focus の設計」があるが、本 Ticket は `yoi panel` の現行 focus UX を具体的に整理する実装単位とする。

## Requirements

- `yoi panel` の user-visible focus model を縮小し、composer target と row selection を中心に整理する。
- composer target は focus ではなく、送信先表示として扱う。
- selected row は、composer が空のときの navigation / Enter 対象として扱う。
- `item action focus` は廃止するか、少なくとも user-visible focus として表示しない。
- `Right action focus` / `Left` の段階的 focus 移動は、必要性を再評価して簡略化する。
- status line / actionbar / key hints が、実際の Enter・矢印・Esc・Tab の挙動と一致するようにする。
- composer に入力済みのテキストを誤って壊したり、暗黙に row action へ送ったりしない。
- Ticket action / Pod open / Companion composer / Ticket Intake target の既存 authority と明示操作は維持する。
- single-Pod TUI の transcript / block navigation はこの Ticket では扱わない。

## Acceptance criteria

- Panel 上で composer target と selected row の意味が分かりやすく表示される。
- `global composer` / `selected row` / `item action` のような user-visible focus 表示が、実際の操作以上に状態を増やして見せない。
- 空 composer と非空 composer で Enter が何をするか、actionbar/status から誤解しにくい。
- `↑/↓`, `Left`, `Right`, `Esc`, `Tab`, `Enter` の key hints が実装と一致している。
- `Right action focus` が残る場合は user-visible focus ではなく、実際に意味のある操作として説明される。不要なら削除・無効化される。
- composer 入力保護が維持される。
- 既存の Ticket action dispatch / Pod open / Intake launch / Companion send の安全性を落とさない。

## Binding decisions / invariants

- `yoi panel` の改善に限定する。
- 通常の single-Pod TUI transcript / block focus navigation は範囲外。
- composer に文字が入っている状態では、通常入力を最優先で保護する。
- composer target は focus ではなく送信先である。
- row selection は、空 composer 時の navigation / Enter 対象である。
- `item action focus` を user-visible focus model から外す。
- Panel は durable state authority にならず、既存の Ticket / Pod authority を維持する。
- Ticket の `ready -> queued` などの明示 user action semantics は変更しない。

## Implementation latitude

- `PanelFocus` の状態名・数・遷移は変更してよい。
- `ItemAction` 相当の状態は削除してよい。
- `Right` / `Left` の扱いは、実装後の単純な model に合わせて削除・no-op・案内表示のいずれかにしてよい。
- `Esc` の詳細挙動は input safety を満たす範囲で調整してよい。
- status line / actionbar / row hints の文言は、実装後の挙動に合わせて整理してよい。
- UX 改善は最小実装でよく、フル navigation mode や vim-like 操作体系は不要。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-ux, input-safety]

## Escalation conditions

- composer 入力保護と row navigation のどちらを優先するかについて、現行 invariant を超える判断が必要になった場合。
- Ticket lifecycle action の明示性や authority boundary を変える必要が出た場合。
- Panel だけでなく single-Pod TUI の navigation model 変更が必要だと判明した場合。

## Validation

- `crates/tui/src/multi_pod.rs` 周辺の focused tests を追加・更新する。
- focus/key handling/status/actionbar の unit tests を通す。
- `cargo test -p tui multi_` またはより focused な同等テストを通す。
- TUI 変更なので完了時に `nix build .#yoi` も確認する。

## Related work

- `00001KSKBPPMR` — TUI: navigation mode / block focus の設計
