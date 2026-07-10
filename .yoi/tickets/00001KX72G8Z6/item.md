---
title: 'Add rich markdown tool rendering to workspace console'
state: 'done'
created_at: '2026-07-10T22:31:21Z'
updated_at: '2026-07-10T22:45:55Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-10T22:31:41Z'
---

## 背景

Workspace Console の Call ブロック化後も、表示は plain text `<pre>` 中心で、TUI の色分け・Edit diff・Markdown/code block 表示が反映されていない。Console は TUI と同じ情報設計に寄せ、Assistant text や tool output を Markdown/HTML として読みやすく表示し、fenced code block は Shiki で highlight する。

## 要件

- TUI tool rendering の色分け方針を Frontend Console に移植する。
- Assistant/tool body を Markdown から安全な HTML に変換して表示する。
- fenced code block は Shiki で syntax highlight する。
- Edit tool は old/new 文字列の diff を専用表示し、削除/追加/文脈行を色分けする。
- Read tool aggregate は引き続き file content を body に表示しない。
- Console model tests と UI/rendering tests を更新する。

## 受け入れ条件

- Console で assistant markdown が HTML として表示される。
- fenced code block が Shiki の highlighted HTML として表示される。
- Console tool blocks に TUI 相当の状態色・tool 色・diff 色が適用される。
- Edit tool が old/new から diff block を生成し、削除行と追加行を別色で表示する。
- Read aggregate は複数 Read をまとめ、file content を表示しない。
- `cd web/workspace && deno task check` と `cd web/workspace && deno task test` が通る。
- `nix build .#yoi` が通る。
