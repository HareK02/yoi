---
id: 20260601-020202-tui-keys-inline-viewport-ui
slug: tui-keys-inline-viewport-ui
title: 'TUI: align insomnia keys UI with inline viewport style'
status: closed
kind: task
priority: P2
labels: [tui, keys, ui]
created_at: 2026-06-01T02:02:02Z
updated_at: 2026-06-01T02:23:12Z
assignee: null
---

## Issue

`insomnia keys` の UI が、現在の TUI 実装全体の見た目・操作感から外れている。特に `tui -r` の Pod/session selection で使っているような inline viewport 型の表示・操作と比べて一貫性が弱く、同じプロダクト内の画面として揃っていない。

このチケットでは、`insomnia keys` の UI を TUI 全体の styling / layout / navigation conventions に合わせる。独自 UI を増やすのではなく、既存 TUI の inline viewport / list / selection / actionbar / key hint の概念に寄せる。

## Requirements

- `insomnia keys` の UI を、`tui -r` のときのような inline viewport 型の画面として再設計・実装すること。
- 色、枠線、余白、選択行、フォーカス、ヘルプ表示、action/status 表示などの styling を、既存 TUI 実装全体の conventions と揃えること。
- key 一覧・選択・詳細表示・操作説明が、他の TUI 画面と同じ mental model で使えること。
- 既存の key 管理機能・保存形式・secret handling の意味論を変えないこと。
- secret 値そのものを不用意に画面、ログ、diagnostics、session history、ticket、test snapshot に出さないこと。
- CLI / non-interactive な key 操作がある場合、それらの既存挙動を壊さないこと。
- TUI 共通部品や style helper が既にある場合は再利用し、`insomnia keys` 専用の重複 styling を増やさないこと。
- 実装前に、現在の `insomnia keys` UI がどの crate/module にあり、`tui -r` inline viewport がどの部品で構成されているかを調査してから修正方針を決めること。

## Acceptance criteria

- `insomnia keys` の画面が、既存 TUI の inline viewport 画面と視覚的・操作的に一貫している。
- key 一覧と選択中 item の表示が、既存の list/viewport styling と同じ規則で描画される。
- 操作キーやヘルプ表示が、他の TUI 画面と同じ提示方法になっている。
- secret 値が accidental に露出しないことを確認している。
- 既存の key 管理フローが regression していないことを、手動確認または focused test で説明できる。
