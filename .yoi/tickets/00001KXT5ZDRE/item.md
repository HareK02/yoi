---
title: 'Run dev workspace process actions detached by default'
state: 'closed'
created_at: '2026-07-18T08:37:37Z'
updated_at: '2026-07-18T08:40:59Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T08:38:38Z'
---

## 背景

`scripts/dev-workspace.sh` は backend/runtime/frontend の stop/start を扱うため、API worker が依存している process を foreground tool call 中に停止すると tool result 永続化前にセッションを壊す可能性がある。

呼び出し元が `start` / `stop` / `restart` を実行した時点では即座に戻り、実際の mutating action は detached scheduled job として後で走るようにする。

## 要件

- `start` / `stop` / `restart` は既定で detached background job として schedule する。
- 既定 delay は 60 秒とし、呼び出し元が tool result を返す時間を確保する。
- scheduled job の pid と log path を呼び出し元に表示する。
- `status` / `help` は同期実行のままにする。
- デバッグ用に foreground 実行へ戻せる環境変数を用意する。
- 実装中に mutating action は実行しない。

## 受け入れ条件

- `bash -n scripts/dev-workspace.sh` が通る。
- `scripts/dev-workspace.sh --help` に detached 既定挙動と override が表示される。
- `scripts/dev-workspace.sh status` は引き続き mutation なしで動く。
- `start` / `stop` / `restart` の実処理分岐は内部 foreground mode 経由に限定され、通常呼び出しでは detached schedule だけを行う。
