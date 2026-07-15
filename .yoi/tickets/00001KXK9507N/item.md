---
title: 'Consolidate Ticket configuration into Workspace settings'
state: 'queued'
created_at: '2026-07-15T16:18:25Z'
updated_at: '2026-07-15T16:19:31Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-15T16:19:31Z'
---

## 背景

`.yoi/ticket.config.toml` は現在、Ticket backend root/provider、Ticket record language、orchestration defaults、固定 role launch config を持っている。Worker Ticket tools は Workspace backend 経由の Ticket authority に寄せ始めており、Ticket 専用 config file が Workspace settings から独立している状態は設定境界として不自然になっている。

local backend の listen address、DB path、runtime store、remote runtime endpoint などの local-only operational settings は project/workspace の Ticket policy とは分けたまま、Ticket に関する durable policy を Workspace settings に統合する。

## 要件

- tracked な Workspace project settings を Ticket configuration の durable authority として導入または再利用する。
- Ticket backend provider/root、Ticket record language、orchestration defaults、固定 role launch config を Workspace settings namespace 配下へ移す。
- local-only backend/runtime settings と tracked project/workspace Ticket policy を混ぜない。
- Ticket CLI、Worker Ticket feature setup、workspace-server Ticket backend endpoint、Panel / role launch code、関連 tests が Workspace settings authority から Ticket settings を解決するように更新する。
- `.yoi/ticket.config.toml` は必要なら移行用に narrow な read-only fallback として読むが、長期的な active authority として残さない。
- workspace-server の Ticket backend endpoint が `.yoi/tickets` を hard-code せず、設定された Ticket backend root を尊重する。

## 受け入れ条件

- Ticket backend root、language、orchestration defaults、role slots の Workspace settings loading を tests で確認している。
- 既存の Ticket CLI と Worker Ticket tools が default では同じ `.yoi/tickets` data に対して動作し続ける。
- Workspace settings で non-default Ticket backend root を設定した場合、direct CLI/local path と workspace-server-backed Worker Ticket tools の両方で尊重される。
- `.yoi/ticket.config.toml` が obsolete/replaced であることを docs または migration note / in-code note に残している。
- affected crates の `cargo test` と `nix build .#yoi` が通る。
