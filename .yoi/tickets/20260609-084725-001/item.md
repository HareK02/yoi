---
title: 'Improve workspace panel display and composer key handling'
state: 'inprogress'
created_at: '2026-06-09T08:47:25Z'
updated_at: '2026-06-09T11:07:23Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-input', 'ux-consistency']
queued_by: 'workspace-panel'
queued_at: '2026-06-09T10:01:08Z'
---

## Background

`yoi panel` のトップ行と actionbar に `Ctrl+T` の操作ヒントが表示されているが、Panel 表示として目立たせる必要が薄く、情報密度も上げている。

また、Panel composer の入力操作が通常のチャット UI / event view TUI composer と一部異なっている。観測された例として、`Ctrl+Left` / `Ctrl+Right` による単語単位カーソル移動が Panel composer では効かない。現在は `InputBuffer` など低レベルの composer 実装は共有されている一方、key handling dispatcher は通常チャット UI と Panel で分かれており、Panel 側が composer editing keymap を十分に共有していない。

Panel は workspace / Ticket 操作の入口である一方、composer は通常の TUI 入力欄として期待されるため、表示と操作の両方を整理する。

## Requirements

- Panel のトップ行から `Ctrl+T` 表示を削除する。
- Panel の actionbar / help text から `Ctrl+T` 表示を削除する。
- Panel の target switching 操作を `Ctrl+T` から `Tab` に変更する。
- `Tab` の優先順位は composer completion がある場合は completion を優先し、completion がない Panel 状態では target switching として扱う。
- Panel composer の編集操作は、通常チャット UI composer と可能な限り互換にする。
- Panel 側で composer 操作を再実装して乖離を増やすのではなく、既存 composer 入力処理・keymap・action model を共有または抽象化し、互換性を維持しやすい構造にする。
- bare letter shortcuts を復活させない。通常の文字入力は composer text として扱う既存方針を維持する。

## Acceptance criteria

- `yoi panel` のトップ行に `Ctrl+T` が表示されない。
- Panel の actionbar / help text に `Ctrl+T` が表示されない。
- Panel で `Tab` により target switching ができる。
- Panel に completion popup / completion state がある場合、`Tab` は target switching より completion 操作を優先する。
- Panel composer で、通常チャット UI composer と同等の主要編集操作が動作する。
  - 例: 左右移動、行頭/行末移動、削除、履歴操作、`Ctrl+Left` / `Ctrl+Right` 等の単語単位移動。
- terminal / backend が特定 key sequence を送らない場合は、その制約を壊れた実装と誤認しない範囲で扱う。
- Panel 専用の ad-hoc composer key handling が増えず、通常 composer との互換性を保ちやすい実装になっている。
- `nix build .#yoi` が通る。

## Binding decisions / invariants

- `Ctrl+T` を Panel の表示要素や target switching 操作として使わない。
- Panel の target switching shortcut は `Tab` とする。
- `Tab` は completion が active な場合、target switching ではなく completion 操作を優先する。
- Composer に入力可能な通常文字キーを Panel 操作用 shortcut として奪わない。
- bare `j` / `k` / `o` / `r` のような bare letter shortcuts は復活させない。
- Panel composer は通常チャット UI composer と別物として再実装しない。既存 composer 操作との互換性を保つ方向で実装する。

## Implementation latitude

- composer 操作の共有方法は実装者が選んでよい。
  - 例: composer input handling の共通関数化。
  - 例: key event normalization の共有。
  - 例: Panel 側を既存 composer action model に寄せる。
- Panel 固有操作は、共通 composer editing layer の上に別レイヤーとして載せてよい。
- トップ行は `Ctrl+T` 削除に加えて、workspace / Ticket / role Pod 状態の表示を整理してよい。ただしこの Ticket の主目的は大規模な Panel redesign ではない。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-input, ux-consistency]

## Escalation conditions

- `Tab` が terminal focus、既存 TUI 操作、accessibility 上の期待、または将来の Panel completion と衝突する場合は、completion 優先の方針を保ったうえで相談する。
- 通常チャット UI composer 側にも同じ操作欠落がある場合は、Panel 固有修正ではなく composer 共通改善として扱う。
- key event が terminal / crossterm 側で区別できない場合は、可能な範囲と制約を implementation report に明記する。

## Validation

- `yoi panel` を起動し、トップ行と actionbar に `Ctrl+T` が出ないことを確認する。
- `Tab` で Panel target switching ができることを確認する。
- completion が active な状態を実装が持つ場合、`Tab` が completion を優先することを確認する。
- Panel composer で通常チャット UI composer と同等の主要 key 操作を手動確認する。
- `nix build .#yoi` を実行する。

## Related work

- `20260606-060548-001` Workspace panel layout and display tuning
- `20260607-213808-001` Remove bare letter shortcuts from workspace panel
- `20260607-001651-002` Workspace panel Companion interface
- `20260601-021104-001` TUI: persist composer input recall history per workspace
