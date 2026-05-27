---
id: 20260527-000011-session-todo-reminder
slug: session-todo-reminder
title: セッション内 Task ツールの注意機構
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:11Z
updated_at: 2026-05-27T00:00:11Z
assignee: null
legacy_ticket: tickets/session-todo-reminder.md
---

## Migration reference

- legacy_ticket: tickets/session-todo-reminder.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# セッション内 Task ツールの注意機構

## 背景

`tickets/session-todo.md` で導入した Task ツール群があっても、LLM はそれを使わずに作業を続け得る。ツールを呼ばないまま会話が長引くと、

- 開始した作業の `inprogress` がずっと放置されたままになる
- 「やったつもり」になって `completed` への更新を忘れる
- そもそも TaskStore の存在を忘れて、構造化を諦めて自由記述に回帰する

OpenCode の todo は専用の注意機構を持たない（汎用 reminder 経由）。一方 Claude Code は `task_reminder` を「N リクエスト無アクティビティで初めて発火するナッジ型」として実装しており、毎リクエスト押し戻しはしない（`<local-agent-install>` の local reminder helper functions、閾値 `a local reminder threshold`）。

Insomnia でも同方針を採り、active Task が残っているのに `TaskCreate` / `TaskUpdate` が一定リクエスト呼ばれていない場合に限り、`<system-reminder>` Item を 1件 history に append する。「やったつもり」抑止と、トークン浪費・LLM の自律性侵害のバランスを取るため、毎リクエスト押し戻しはしない。

## 前提

- `tickets/session-todo.md` の TaskStore と `TaskCreate` / `TaskUpdate` / `TaskList` / `TaskGet` ツールが利用可能
- `Interceptor::pending_history_appends` レーンが利用可能（`tickets/notify-history-persist.md` で導入済み）

## 方針

- **`pending_history_appends` で実装**。発火時に `<system-reminder>` ブロックを含む新規 system message Item を返し、Worker が `worker.history` に append する。Notify / PodEvent と同じレーンで永続化・resume・compaction が自動で揃う
- **揮発的注入は採らない**（`AGENTS.md` 「LLM コンテキストの加工原則」で禁止。history に commit せずに context を変えると、resume 時に LLM の発言の根拠が再現できなくなる）
- **system-reminder 注入機構の汎用化はやらない**。利用者が Task 1機構しかない段階で抽象を立てない（`AGENTS.md`「概念の追加は不在が問題になってから」）。タグ形式 `<system-reminder>...</system-reminder>` の規約は本実装で踏襲する
- **発火はナッジ型**。N リクエスト無アクティビティで初めて発火し、cooldown も持つ

## 要件

### Interceptor

- `pending_history_appends` で以下を **AND** で満たす場合のみ発動し、`<system-reminder>` ブロックを含む `Item::system_message` を 1件返す。条件外なら空 `Vec<Item>` を返す
  - active Task（`pending` または `inprogress`）が 1件以上存在する
  - 直近 N リクエスト（暫定 N=8）`TaskCreate` / `TaskUpdate` のいずれも呼ばれていない
  - 前回 reminder Item の append から M リクエスト（暫定 M=8）以上経過
- ここで言う「リクエスト」は LLM への 1回の推論呼び出し（assistant 応答 1回）の単位。1ユーザー発火内で tool ループが回れば、`tool_result` を受けて発火する次のリクエストもそれぞれ 1としてカウントする
- カウンタは Pod 側の session-lifetime 状態として保持する（`requests_since_last_task_management` / `requests_since_last_reminder`）。resume 時は worker.history の逆走査で再計算するか 0 リセットで再開する。後者でも「初回ナッジが最大 N リクエスト遅れる」だけで挙動として致命ではない
- 返す Item の本文は `<system-reminder>` で囲み、現在の active Task を `taskid` / `status` / `subject` を含む簡潔な形式で列挙する。`description` は長大化を避けるため省略してよい
- active Task が空の場合は何も append しない（思い出させる対象が無いなら不要）

## 完了条件

- 直近 N リクエスト連続で `TaskCreate` / `TaskUpdate` が呼ばれず、かつ active Task が残っている場合に限り、`pending_history_appends` が `<system-reminder>` を含む `Item::system_message` を 1件返す
- 返された Item が `worker.history` に append され、その後のリクエスト・`history.json`・resume 後の `get_history` でも同じ Item が見える（揮発レーンは持たない）
- `TaskCreate` / `TaskUpdate` のいずれかが呼ばれるとカウンタがリセットされ、再び N リクエスト経過するまでは reminder が出ない
- reminder が一度出たあとは、cooldown M リクエストが経過するまで再注入されない
- active Task が 0件の場合は reminder が出ない
- 単体テストで Interceptor の発火条件（リクエスト回数閾値、active 0件、cooldown）がカバーされる

## 範囲外

- inprogress 滞留検出 / 多重 inprogress 検出など、状態異常ベースの追加トリガ（必要になれば別チケットで追加）
- system-reminder 注入機構の汎用化（`TODO.md` に立項済み、別途検討）
- `TaskCreate` / `TaskUpdate` の戻り値に active Task 全件を埋め込む強化（必要に応じて Tool ticket 側で対応）
- サブエージェント / sidechain での独自 reminder 発火（main Pod の interceptor から動く構造のため自然に対象外）

## 参照

- 設計指針: `AGENTS.md`（LLM コンテキストの加工原則。揮発的注入は禁止、history に append してから commit する）
- 前提: `tickets/session-todo.md`（Tool 群と TaskStore）、`tickets/notify-history-persist.md`（`pending_history_appends` レーン）
- 参考実装: Claude Code の `task_reminder`（local reminder helper functions、閾値 `a local reminder threshold`）
