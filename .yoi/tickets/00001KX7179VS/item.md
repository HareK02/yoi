---
title: 'Improve workspace console tool call rendering'
state: 'inprogress'
created_at: '2026-07-10T22:08:58Z'
updated_at: '2026-07-10T22:09:21Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-10T22:09:21Z'
---

## 背景

Workspace frontend の Worker Console では、tool call と tool result が別々の行として表示され、streaming 中も `tool call · Bash` / `tool call done · Bash` / `tool result` のように分断されて見える。TUI では tool call を `Call` ブロックとしてまとめ、状態更新と結果を同一ブロック内で表示しているため、Frontend Console も同じ情報設計に寄せる。

## 要件

- Workspace Console の tool call 表示を、call 単位の1ブロックに集約する。
- streaming / done / failed などの状態変化は同じブロック内で更新して見せる。
- tool result は対応する tool call ブロックに取り込み、独立した `tool result` 行として重複表示しない。
- TUI の汎用 tool 表示と、Bash / Read / Write / Edit / Glob / Grep / WebFetch / WebSearch 等の専用表示方針を確認し、Frontend 側でも少なくとも Bash など主要 tool の要約表示を揃える。
- call id が取れない・対応する call が見つからない result は、安全な fallback 表示を維持する。

## 受け入れ条件

- Console で `Bash { command: "pwd" }` の tool call と result が1つの Call ブロックとして表示される。
- streaming 中の tool call は同じブロックで `streaming` 状態を表示し、完了時に同じブロックが完了状態へ更新される。
- tool result の content が対応する Call ブロック内に表示される。
- 汎用 tool では tool 名、引数、状態、結果を同一ブロック内で確認できる。
- Bash など主要 tool では TUI と同じ方針の専用 summary が表示される。
- Console model の unit test で tool call/result の集約を確認する。
- `cd web/workspace && deno task check` と `cd web/workspace && deno task test` が通る。
