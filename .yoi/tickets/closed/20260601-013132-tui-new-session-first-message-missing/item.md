---
id: 20260601-013132-tui-new-session-first-message-missing
slug: tui-new-session-first-message-missing
title: 'TUI: first message missing when starting a new session'
status: closed
kind: bug
priority: P1
labels: [tui, session, display]
created_at: 2026-06-01T01:31:32Z
updated_at: 2026-06-01T02:23:11Z
assignee: null
legacy_ticket: null
---

## Issue

TUI で新しいセッションを開始して会話を始めたとき、ユーザーが最初に送ったメッセージが会話ビューに表示されないことがある。

この問題は、ユーザー入力が Pod / Worker に届いて処理されているかどうかとは独立に、少なくとも TUI の表示上「最初の user message が欠落したように見える」不具合として扱う。新規セッション開始時の初回 turn だけで起きる可能性が高く、既存セッションへの追加発話や 2 通目以降の表示と異なる初期化順序・履歴同期・描画更新経路が疑わしい。

## Requirements

- 新しいセッションで最初のユーザーメッセージを送信した直後、そのメッセージが TUI の会話ビューに表示されること。
- 表示は、Pod / Worker の履歴に記録された user message と対応していること。表示だけの一時的な偽メッセージで履歴不整合を隠さないこと。
- 既存セッションに attach / restore した場合の履歴表示を壊さないこと。
- 2 通目以降の通常送信、running 中の after-run queue、既存の composer 入力履歴の挙動を変えないこと。
- 原因が TUI 側の view model 初期化、Pod attach / run 開始時の snapshot、history append、または描画更新のどこにあるかを調査で切り分けること。
- 修正に進む場合は、初回メッセージ表示を再現・検証できるテストまたは明確な手動確認手順を残すこと。
