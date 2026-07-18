---
title: 'Document dev workspace process script usage'
state: 'closed'
created_at: '2026-07-18T10:09:26Z'
updated_at: '2026-07-18T10:10:43Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T10:09:33Z'
---

## 背景

`scripts/dev-workspace.sh` は backend/runtime/frontend を停止・再起動する開発用スクリプトで、誤って foreground 実行や frontend restart を行うと API セッションやブラウザ接続に影響する。スクリプト本体を読むだけで安全な使い方と注意が分かるようにする。

## 要件

- スクリプト先頭コメントに基本コマンドを記載する。
- `start` / `stop` / `restart` が既定で detached schedule されることを明記する。
- `restart` は frontend を触らないことを明記する。
- frontend も付け替える時は `start` / `stop` を使う注意を書く。
- foreground override は通常使わない注意を書く。

## 受け入れ条件

- `scripts/dev-workspace.sh` の先頭を読めば使い方と注意が分かる。
- `bash -n scripts/dev-workspace.sh` が通る。
- `scripts/dev-workspace.sh --help` が通る。
- mutating action は実行しない。
