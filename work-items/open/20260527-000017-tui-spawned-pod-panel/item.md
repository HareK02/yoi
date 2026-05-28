---
id: 20260527-000017-tui-spawned-pod-panel
slug: tui-spawned-pod-panel
title: TUI: spawned child Pod の一覧と一時 attach
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:17Z
updated_at: 2026-05-28T14:16:02Z
assignee: null
legacy_ticket: tickets/tui-spawned-pod-panel.md
---

## Migration reference

- legacy_ticket: tickets/tui-spawned-pod-panel.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# TUI: spawned child Pod の一覧と一時 attach

## 背景

insomnia の開発では、親 Pod が複数の実装 Pod / reviewer Pod を spawn し、並列に作業させる運用が増えている。現在、spawned child の状態確認や出力確認は主に tool (`ListPods`, `ReadPodOutput`, `SendToPod`, `StopPod`) 経由で行っているが、TUI 上では親 Pod の会話と child Pod の進捗を行き来しにくい。

ネイティブ GUI は将来的には便利だが、現時点で必要なタスクではない。まず TUI のまま、現在の Pod が spawn した child Pod を一覧し、一時的に attach / view できる UI を用意したい。

## Prerequisite

- `20260528-141602-tui-pod-list-view-abstraction`

This ticket should build on the shared TUI Pod list/view abstraction instead of introducing a separate child-Pod-specific list model. The child panel may specialize the source/visibility to current-parent spawned children, but row status, reachability diagnostics, attach target representation, selection, and refresh behavior should reuse the prerequisite abstraction.

## 要件

- TUI 上で、現在の Pod が spawn した child Pod を一覧できる。
  - source は spawned child registry / Pod state persistence を使う。
  - ホスト上の全 Pod を無条件に見せる UI にはしない。
  - current parent から見える child Pod だけを対象にする。
- 各 child row には最低限以下を表示する。
  - pod name
  - alive / stopped / unreachable などの状態
  - delegated scope の概要
  - 最終更新時刻または最終出力時刻（取得できる範囲）
  - 未読出力の有無または最終 assistant text preview（可能なら）
- TUI から child Pod に一時 attach / view できる。
  - 親 Pod の TUI を完全に終了せず、child の履歴 / streaming 出力を確認できる。
  - 戻る操作で親 Pod view に戻れる。
  - 最小実装では read-only view でもよい。child へ入力を送る操作は後続でもよい。
- child view 中でも、どの Pod を見ているか視覚的に分かる。
  - status line / title / breadcrumb など。
- child が stopped / unreachable の場合は明確に表示し、attach 失敗を診断する。
- 既存 tool の `ListPods` / `ReadPodOutput` / `SendToPod` / `StopPod` の意味を変えない。
- visibility は parent-child 関係に基づけ、Pod discovery の global list と混ぜない。

## 操作案

詳細 keybinding は実装時に確定する。

候補:

- command mode から `:pods` で child Pod list を開く。
- list 上で Enter すると child view へ一時 attach。
- `Esc` / `b` / command で parent view へ戻る。
- child view から `:send` などで入力する機能は後続 ticket にしてよい。

## 完了条件

- 親 Pod の TUI で spawned child Pod の一覧を表示できる。
- live child Pod を選択すると、その child の snapshot / streaming output を TUI 上で確認できる。
- parent view に戻れる。
- stopped / unreachable child は一覧上で状態が分かり、attach 失敗が診断される。
- ホスト全 Pod ではなく、parent から見える child Pod だけが対象である。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p tui -p pod -p protocol`

## 範囲外

- ネイティブ GUI クライアント。
- 複数 Pod view の同時分割表示。
- child Pod への full interactive input。
- child Pod の自動再起動。
- host-wide Pod browser。
- Pod discovery tool の visibility model 変更。
