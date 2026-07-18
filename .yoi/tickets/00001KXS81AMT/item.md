---
title: 'Compactionのsession探索toolsをSessionReferenceViewへ移行する'
state: 'closed'
created_at: '2026-07-17T23:54:22Z'
updated_at: '2026-07-18T00:02:34Z'
assignee: null
---

## 背景

`SessionReferenceView` は extract / compact の共通基盤として導入されたが、直後の実装では compaction の `search_session_log` のみが view を使い、`read_session_items` は旧 compact 専用 formatter / direct item traversal のままだった。

既存 compact も explore tool 群を利用するものに寄せるため、この Ticket では compaction の session read/search tools を `SessionReferenceView` へ統合する。

## 実装要件

- compaction の `search_session_log` は `SessionReferenceView::search` を使う。
- compaction の `read_session_items` は `SessionReferenceView::read` を使う。
- `read_session_items` の existing `mode = compact | full` semantics は維持する。
  - compact: tool arguments / tool result content を omitted にする。
  - full: bounded に tool arguments / tool result content を読める。
- 旧 compact 専用の session search / format helper を削除する。
- `SessionReferenceView` 側に read detail mode を追加し、compact / full を表現できるようにする。
- 既存 compact tests を維持または更新する。

## 非目標

- compaction prompt を大きく変えない。
- compact worker の tool 名を変更しない。
- extract の `session-explore` feature 本体はこの Ticket では実装しない。

## 受け入れ条件

- `search_session_log` と `read_session_items` がどちらも `SessionReferenceView` を経由する。
- compact mode で raw tool content が出ない。
- full mode で bounded tool content が読める。
- `cargo test -p worker compact::worker` が通る。
- `cargo test -p worker` が通る。
- code 変更として `nix build .#yoi` が通る。
