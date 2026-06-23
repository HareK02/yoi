---
title: 'Improve Workspace web ticket Kanban grouping and lazy rows'
state: 'ready'
created_at: '2026-06-23T05:50:36Z'
updated_at: '2026-06-23T05:51:34Z'
assignee: null
---

## 背景

Workspace web の Repository Ticket Kanban は Ticket state ごとに縦方向へ列/行を増やして表示しているが、`closed` など件数の多い state が画面を圧迫しやすい。実運用では `planning` と `ready`、`queued` と `inprogress` は近い段階としてまとめて見たい一方で、各 group 内では実行に近い state を上に表示したい。

Repository page の Ticket 表示を、state group ごとの独立スクロール領域と lazy row loading に変更する。必要に応じて Kanban 表示コンポーネントを切り出し、page component の責務を軽くする。

## 要件

### State grouping / sort

- `planning` と `ready` を同じ表示 group に統合する。
  - 表示 label は実装時に自然なものを選んでよいが、両 state が同じ group だと分かること。
  - group 内の sort は `ready` が `planning` より上に来ること。
- `queued` と `inprogress` を同じ表示 group に統合する。
  - group 内の sort は `inprogress` が `queued` より上に来ること。
- その他の state は既存意味を保つ。
  - 例: `done`, `closed` は独立 group のままでよい。
- 各 Ticket row には元の `state` が識別できる表示を残す。

### Per-group independent scroll

- すべての Kanban group / row list は独立した scroll area を持つ。
- 各 scroll area の高さはおおむね Ticket 10件分を表示できる高さにする。
- 件数の多い state/group が page 全体を圧迫しないこと。
- page 全体の scroll と group 内 scroll の責務が不自然に競合しないこと。

### Lazy row loading

- 各 group は初期表示 30件までに制限する。
- group 内 scroll area の下部付近まで到達したら、その group だけ追加件数を読み込んで表示する。
- lazy loading は group ごとに独立していること。
  - `closed` を下まで scroll しても `ready/planning` など他 group の表示件数は増えない。
- Backend API の pagination 実装までは必須にしない。
  - 既存 API がまとめて返す範囲内で、Frontend 側の表示件数を段階的に増やす実装でよい。
  - ただし将来 backend pagination に差し替えられるよう、表示 state は group 単位で管理する。

### Component boundary

- Kanban 表示は `WorkspacePage.svelte` から切り出してよい。
- 切り出す場合は、少なくとも以下の責務を component に寄せる。
  - state grouping
  - group 内 sort
  - per-group visible count
  - scroll bottom detection
  - Ticket row rendering
- page component は repository data fetch / high-level layout に寄せる。

### Design system alignment

- Workspace web design system に沿い、不要な角丸/縁取り/filled card を増やさない。
- group 境界は余白・文字サイズ・文字色・細い rule を優先して表現する。
- 色は `web/workspace/src/app.css` の token を使い、raw color を追加しない。

## Non-goals

- Backend pagination API の本格実装。
- Ticket state lifecycle の変更。
- Ticket mutation UI の追加。
- Repository target metadata / DB schema の実装。
- TUI Dashboard / Panel の Kanban 表示変更。

## 受け入れ条件

- Repository Ticket Kanban で `planning` と `ready` が同一 group として表示される。
- その group 内で `ready` Ticket が `planning` Ticket より上に表示される。
- Repository Ticket Kanban で `queued` と `inprogress` が同一 group として表示される。
- その group 内で `inprogress` Ticket が `queued` Ticket より上に表示される。
- 各 group は独立 scroll area を持ち、おおむね 10件分の高さでスクロールできる。
- 各 group は初期 30件だけ表示し、下部まで scroll するとその group だけ追加表示される。
- 件数の多い `closed` group が page 全体の縦幅を大きく圧迫しない。
- Ticket row には元 state が分かる表示がある。
- 必要な場合、Kanban 表示が再利用可能な Svelte component に切り出されている。
- `cd web/workspace && deno task check && deno task build`、`git diff --check`、`nix build .#yoi --no-link` が通る。
