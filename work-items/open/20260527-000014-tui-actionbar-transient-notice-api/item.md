---
id: 20260527-000014-tui-actionbar-transient-notice-api
slug: tui-actionbar-transient-notice-api
title: TUI: actionbar transient notice API
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:14Z
updated_at: 2026-05-27T00:00:14Z
assignee: null
legacy_ticket: tickets/tui-actionbar-transient-notice-api.md
---

## Migration reference

- legacy_ticket: tickets/tui-actionbar-transient-notice-api.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# TUI: actionbar transient notice API

## 背景

TUI の actionbar は最下部の補助表示行として、現在の mode や一時的な操作フィードバックを出す場所になりつつある。

一方で、現在は `Ctrl-C` の二段階終了 guard のような一時通知も `app.push_error(...)` 等で view 上に残る message として扱われている。これは後から見返すログではなく、数秒だけ見えれば十分な操作フィードバックである。

また、memory audit log 実装では extract / consolidation worker の直近 event を actionbar に表示する予定であり、個別機能ごとに ad hoc な actionbar 表示を増やすと優先順位・寿命・表示競合の扱いが散らばる。

## 方針

Actionbar を「history / transcript に残さない transient UI state」の共通表示面として扱う API を App 側に用意する。

永続的に残すべき Pod event / model output / tool result / user-visible error と、一時的な操作フィードバックを分離する。actionbar notice は UI の補助表示であり、LLM context や session history へ暗黙注入しない。

## 要件

- App に actionbar transient notice を設定・期限切れ・取得するための API を追加する。
  - 例: `flash_actionbar_notice(text, duration)` または `set_actionbar_notice(...)`
  - notice には最低限 `text`, `level`, `source`, `expires_at` 相当を持たせる。
  - time source はテストしやすい形にする。
- actionbar rendering は transient notice を優先表示できる。
  - 既存の command mode marker、queued input hint、scroll indicator、view mode label と競合しない優先順位を定義する。
  - notice が期限切れなら表示しない。
- `Ctrl-C` の二段階終了 guard の表示を view log から actionbar notice に移す。
  - `Pod keeps running` などの一時説明は transcript/view 上に残さない。
  - 二度押しの挙動自体は変えない。
- memory worker の actionbar 表示が既に実装済みの場合、可能な範囲でこの API に寄せる。
  - 未実装・別 branch 上の場合は、この ticket の範囲では API 設計が衝突しないようにする。
- actionbar notice は通常の LLM context に暗黙注入しない。
  - 必要な正本ログは各機能の audit/session log に残す。

## 完了条件

- actionbar transient notice 用 API が App/UI に追加されている。
- `Ctrl-C` 二段階終了 guard の一時メッセージが actionbar に表示され、view log には残らない。
- notice の期限切れと優先表示の挙動がテストされている。
- 既存の command mode / queued input / scroll / view mode actionbar 表示が破綻していない。
- `cargo fmt --check` と関連 TUI テストが通る。

## 範囲外

- actionbar の複数行化。
- 汎用 notification center / viewer UI。
- Pod / worker の正本ログ形式の変更。
- memory audit log 本体の実装。
