---
id: 20260601-021104-tui-composer-history-persistence
slug: tui-composer-history-persistence
title: TUI: persist composer input recall history per workspace
status: open
kind: task
priority: P2
labels: [tui, composer, history, persistence]
created_at: 2026-06-01T02:11:04Z
updated_at: 2026-06-01T02:11:04Z
assignee: null
legacy_ticket: null
---

## Issue

TUI composer では上下キーで過去に送信・queue した入力を recall できるが、現在の履歴は TUI-local な揮発状態に留まっている。新しく TUI を起動し直した後も、同じ workspace で使った composer input history を呼び出せるようにしたい。

既存決定として、composer input history recall は Pod protocol / transcript / session history を変更しない TUI-local editing affordance である。この意味論は維持したまま、保存先だけを user data として永続化する。

## Storage decision

永続化先は workspace 配下の `./.insomnia/` ではなく、ユーザー data dir（既定では `~/.insomnia`、実装上は既存の data-dir 解決に従う）を使う方針とする。

理由:

- composer input history は個人の操作履歴であり、project-authored asset ではない。
- 入力には secret / private context が混ざり得るため、workspace に書くと git 追跡・共有・公開監査のリスクが上がる。
- `./.insomnia/` は workflow / knowledge / manifest assets など project/workspace 側の明示的な資産に寄せ、生成された個人履歴は user data 側に置く方が境界が明確。
- 「どの workspace の履歴か」は、user data 側で workspace identity（canonical workspace root / git root などから作る stable key）と display metadata を持てば表現できる。

## Requirements

- 上下キーで呼び出す composer input history を、TUI 再起動後も利用できるよう永続化すること。
- 履歴は workspace ごとに分離すること。別 workspace の入力履歴が通常操作で混ざらないこと。
- 保存先は既存の user data dir 配下にすること。デフォルト表示としては `~/.insomnia` 相当だが、実装は data-dir override / path resolution に従うこと。
- `./.insomnia/` には composer history を作成しないこと。
- 保存 record は、どの workspace の履歴か判別できる metadata / key を持つこと。
- 既存の TUI-local / non-destructive recall semantics を維持すること。
  - Pod protocol を変えない。
  - transcript / session history を mutate しない。
  - recalled input は、ユーザーが再送信するまでは conversation state に影響しない。
- 入力は typed `Segment` vector として保存し、structured input の recall を壊さないこと。
- non-blank input のみ保存し、連続重複を抑止し、履歴件数は bounded にすること。既存挙動の 100 件 bound を尊重する。
- secret 値が混ざり得る前提で、保存内容を diagnostics / logs / tickets / tests snapshot / model context に不用意に出さないこと。
- 破損した履歴ファイルがあっても TUI startup を致命的に壊さず、必要なら warning と空履歴 fallback にすること。
- 既存の multiline cursor navigation、Up/Down browse、draft restore、edit-on-recall の挙動を regression させないこと。

## Acceptance criteria

- 同じ workspace で TUI を再起動しても、以前送信した composer input を上下キーで recall できる。
- 別 workspace では履歴が分離される。
- workspace 配下に新しい composer history file が作られない。
- user data dir 配下に workspace-scoped な composer history が保存される。
- Pod session log / Worker history / transcript には、recall 操作だけでは何も追加されない。
- focused test または明確な手動確認手順で、永続化・workspace 分離・既存 recall semantics を検証できる。
