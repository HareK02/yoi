---
id: 20260528-141602-tui-pod-list-view-abstraction
slug: tui-pod-list-view-abstraction
title: TUI Pod list/view abstraction
status: open
kind: task
priority: P2
labels: [tui, pod, architecture]
created_at: 2026-05-28T14:16:02Z
updated_at: 2026-05-28T14:16:02Z
assignee: null
legacy_ticket: null
---

## Background

TUI で扱う Pod 関連 UI は、少なくとも次の二つの後続 ticket から使われる。

- `20260527-000017-tui-spawned-pod-panel`: spawned child Pod の一覧と一時 attach。
- `20260527-000023-multi-pod-view-ui`: 複数 Pod view を行き来する UI。

両者は表示対象や操作範囲が異なる一方で、Pod の一覧取得、status 表示、visible / attachable 判定、row 表示、選択状態、view 切り替えの土台を共有する。これを各 ticket が個別に実装すると、TUI 内で Pod list / picker / view 管理が重複し、visibility model や attach 診断がずれやすい。

まず TUI 内で用いる複数 Pod の list/view model を抽象化し、後続 UI が同じ情報構造と操作プリミティブを使える状態にする。

## Requirement

- TUI が Pod 一覧 UI を構成するための共通 model / state / helper を用意する。
  - Pod name
  - source / visibility kind（例: current parent から見える spawned child、restore picker 由来、将来の multi-view 対象など）
  - live reachability / `PodStatus`
  - socket path / attach target
  - delegated scope summary または表示可能な metadata
  - stopped / unreachable / missing state の診断情報
- list row rendering / selection / refresh の責務境界を整理する。
  - TUI widget は表示と選択に寄せる。
  - Pod discovery / client protocol / registry state の取得詳細を UI 表示ロジックへ直接散らさない。
- child Pod panel と multi-Pod view UI が同じ抽象を使える設計にする。
- visibility model は変えない。
  - host-wide Pod browser を作らない。
  - spawned child panel は current parent から見える child Pod のみを対象にする。
  - multi-Pod view UI も、具体要件が決まるまではこの抽象に新しい可視範囲を勝手に足さない。
- 既存の `ListPods` / `ReadPodOutput` / `SendToPod` / `StopPod` tool semantics は変えない。
- 既存の TUI resume picker / attach flow を壊さない。

## Acceptance criteria

- TUI crate 内に、複数 Pod list/view UI で再利用できる typed abstraction がある。
- spawned child Pod list と multi-Pod view UI の後続実装が、その abstraction を使う前提で説明できる。
- Pod row の status / reachability / attach target / diagnostic 表示に必要な情報が一箇所の model にまとまっている。
- visibility scope は caller が明示的に渡すか、source kind として表現され、UI helper が host-wide enumeration を暗黙に行わない。
- 既存 picker / attach 関連テスト、または新規 unit test で list model / selection / status mapping の基本挙動が確認されている。
- `cargo fmt --check`
- `cargo check -p tui -p client -p pod`
- 必要に応じて `cargo test -p tui -p pod -p protocol`

## Relationship

This is a prerequisite for:

- `20260527-000017-tui-spawned-pod-panel`
- `20260527-000023-multi-pod-view-ui`

## Out of scope

- spawned child Pod panel の完成。
- 複数 Pod view UI の完成。
- child Pod への interactive input。
- host-wide Pod browser。
- Pod discovery / permission / registry visibility model の変更。
- native GUI。
